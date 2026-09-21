use anyhow::Context;
use windows::core::Interface;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind,
    GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObjectEx};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use crate::d3d::D3d;
use crate::renderer::SourceTexture;

const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
const BUFFERS: i32 = 2;

/// Read-only capture of one monitor through the compositor – the same path OBS
/// and the Xbox Game Bar use. Nothing here touches any other process.
pub struct MonitorCapture {
    _item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    device: IDirect3DDevice,
    size: SizeInt32,
    /// Auto-reset Win32 event, signalled from the pool's worker thread whenever
    /// a frame arrives, so the render loop can sleep instead of polling.
    frame_event: HANDLE,
    frame_arrived_token: i64,
}

/// A captured frame. Keeps the WGC frame alive so the pool cannot recycle the
/// texture while the caller is still copying from it; drop it right after.
pub struct CapturedFrame {
    _frame: Direct3D11CaptureFrame,
    pub texture: ID3D11Texture2D,
    pub width: i32,
    pub height: i32,
}

impl MonitorCapture {
    pub fn new(device: IDirect3DDevice, monitor_handle: isize) -> anyhow::Result<Self> {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem =
            unsafe { interop.CreateForMonitor(HMONITOR(monitor_handle as *mut _)) }.context("CreateForMonitor")?;
        let size = item.Size()?;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&device, FORMAT, BUFFERS, size)?;
        let frame_event = unsafe { CreateEventW(None, false, false, None) }?;
        let event_value = frame_event.0 as isize;
        let frame_arrived_token = pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
            unsafe { SetEvent(HANDLE(event_value as *mut _)) }?;
            Ok(())
        }))?;
        let session = pool.CreateCaptureSession(&item)?;
        session.SetIsCursorCaptureEnabled(false)?;

        // Same sequence OBS uses to get rid of the yellow capture border on Windows 11.
        match GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
            .and_then(|op| op.join())
        {
            Ok(status) => println!("borderless access: {status:?}"),
            Err(e) => println!("borderless access request failed ({e}) – a border may be visible"),
        }
        if let Err(e) = session.SetIsBorderRequired(false) {
            println!("IsBorderRequired=false rejected ({e}) – a border may be visible");
        }

        session.StartCapture()?;
        Ok(Self { _item: item, pool, session, device, size, frame_event, frame_arrived_token })
    }

    /// Waitable handle that becomes signalled when at least one frame is pending.
    pub fn frame_event(&self) -> HANDLE {
        self.frame_event
    }

    /// The newest frame, or `None` when nothing new arrived since the last call.
    /// A resolution change recreates the pool and skips that one frame.
    pub fn try_next_frame(&mut self) -> anyhow::Result<Option<CapturedFrame>> {
        let frame = match self.pool.TryGetNextFrame() {
            Ok(frame) => frame,
            // windows-rs maps a null WinRT return into an empty error (HRESULT 0).
            Err(e) if e.code().is_ok() => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let content = frame.ContentSize()?;
        if content.Width != self.size.Width || content.Height != self.size.Height {
            println!("source size changed to {}x{} – recreating frame pool", content.Width, content.Height);
            self.size = content;
            self.pool.Recreate(&self.device, FORMAT, BUFFERS, content)?;
            return Ok(None);
        }
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
        Ok(Some(CapturedFrame { _frame: frame, texture, width: content.Width, height: content.Height }))
    }
}

impl Drop for MonitorCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.RemoveFrameArrived(self.frame_arrived_token);
        let _ = self.pool.Close();
        unsafe {
            let _ = CloseHandle(self.frame_event);
        }
    }
}

/// One frame of `monitor_handle` as a fresh texture, via a temporary capture
/// session. A monitor that yields nothing within `timeout_ms` (e.g. one that
/// is asleep) gives a black texture so selection still works there.
pub fn grab_one_frame(d3d: &D3d, monitor_handle: isize, width: u32, height: u32, timeout_ms: u32) -> anyhow::Result<SourceTexture> {
    let mut capture = MonitorCapture::new(d3d.winrt_device()?, monitor_handle)?;
    unsafe {
        let _ = WaitForSingleObjectEx(capture.frame_event(), timeout_ms, false);
    }
    let mut newest = None;
    while let Some(frame) = capture.try_next_frame()? {
        newest = Some(frame);
    }
    match newest {
        Some(frame) if frame.width as u32 == width && frame.height as u32 == height => {
            let texture = SourceTexture::new(&d3d.device, width, height)?;
            texture.copy_from(&d3d.context, &frame.texture);
            Ok(texture)
        }
        _ => SourceTexture::black(&d3d.device, width, height),
    }
}
