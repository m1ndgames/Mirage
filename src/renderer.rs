use anyhow::{anyhow, bail};
use windows::core::{s, Interface, PCSTR};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RenderTargetView, ID3D11SamplerState,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BUFFER_DESC, D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_SAMPLER_DESC, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    IDXGISwapChain1, IDXGISwapChain2, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;

use crate::d3d::D3d;
use crate::geometry::Rect;

const SHADER_SOURCE: &str = include_str!("shaders.hlsl");

/// A GPU-resident copy of a captured frame that shaders can sample.
pub struct SourceTexture {
    pub texture: ID3D11Texture2D,
    pub srv: ID3D11ShaderResourceView,
    pub width: u32,
    pub height: u32,
}

impl SourceTexture {
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> anyhow::Result<Self> {
        Self::create(device, width, height, None)
    }

    /// Zero-initialised – the only CPU-side pixel data in Mirage, used when a
    /// monitor yields no frame in time during selection.
    pub fn black(device: &ID3D11Device, width: u32, height: u32) -> anyhow::Result<Self> {
        let zeros = vec![0u8; (width * height * 4) as usize];
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: zeros.as_ptr() as *const _, SysMemPitch: width * 4, SysMemSlicePitch: 0 };
        Self::create(device, width, height, Some(&init as *const _))
    }

    fn create(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        init: Option<*const D3D11_SUBRESOURCE_DATA>,
    ) -> anyhow::Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, init, Some(&mut texture)) }?;
        let texture = texture.ok_or_else(|| anyhow!("no texture"))?;
        let mut srv = None;
        unsafe { device.CreateShaderResourceView(&texture, None, Some(&mut srv)) }?;
        Ok(SourceTexture { texture, srv: srv.ok_or_else(|| anyhow!("no shader resource view"))?, width, height })
    }

    /// GPU copy; `frame` must have the same size and format.
    pub fn copy_from(&self, ctx: &ID3D11DeviceContext, frame: &ID3D11Texture2D) {
        unsafe { ctx.CopyResource(&self.texture, frame) };
    }
}

/// Flip-model swap chain plus render target view for one window.
pub struct SwapChainTarget {
    swap_chain: IDXGISwapChain2,
    frame_latency_waitable: HANDLE,
    rtv: ID3D11RenderTargetView,
    width: u32,
    height: u32,
}

impl SwapChainTarget {
    pub fn new(d3d: &D3d, hwnd: HWND, width: u32, height: u32) -> anyhow::Result<Self> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            ..Default::default()
        };
        let swap_chain: IDXGISwapChain1 =
            unsafe { d3d.factory.CreateSwapChainForHwnd(&d3d.device, hwnd, &desc, None, None) }?;
        let swap_chain: IDXGISwapChain2 = swap_chain.cast()?;
        unsafe {
            swap_chain.SetMaximumFrameLatency(1)?;
            d3d.factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
        }
        let frame_latency_waitable = unsafe { swap_chain.GetFrameLatencyWaitableObject() };
        let back_buffer: ID3D11Texture2D = unsafe { swap_chain.GetBuffer(0) }?;
        let mut rtv = None;
        unsafe { d3d.device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv)) }?;
        Ok(SwapChainTarget {
            swap_chain,
            frame_latency_waitable,
            rtv: rtv.ok_or_else(|| anyhow!("no render target view"))?,
            width,
            height,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Blocks until the swap chain can accept another frame (at most ~1 s).
    /// On the dev machine this never blocks (PLAN.md, M0 results) – pacing is
    /// event driven; this is only a safety net.
    pub fn wait_for_frame_slot(&self) {
        unsafe {
            let _ = WaitForSingleObjectEx(self.frame_latency_waitable, 1000, false);
        }
    }

    pub fn clear(&self, ctx: &ID3D11DeviceContext) {
        unsafe {
            ctx.ClearRenderTargetView(&self.rtv, &[0.0, 0.0, 0.0, 1.0]);
            ctx.OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
        }
    }

    pub fn present(&self) -> anyhow::Result<()> {
        unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)) }.ok()?;
        Ok(())
    }
}

/// Shaders and state shared by every window. One per device.
pub struct Pipeline {
    vs: ID3D11VertexShader,
    ps_mirror: ID3D11PixelShader,
    ps_overlay: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    crop_buffer: ID3D11Buffer,
    overlay_buffer: ID3D11Buffer,
}

impl Pipeline {
    pub fn new(d3d: &D3d) -> anyhow::Result<Self> {
        let vs_blob = compile(s!("vs_main"), s!("vs_5_0"))?;
        let ps_blob = compile(s!("ps_main"), s!("ps_5_0"))?;
        let ov_blob = compile(s!("ps_overlay"), s!("ps_5_0"))?;
        let mut vs = None;
        let mut ps_mirror = None;
        let mut ps_overlay = None;
        unsafe {
            d3d.device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vs))?;
            d3d.device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut ps_mirror))?;
            d3d.device.CreatePixelShader(blob_bytes(&ov_blob), None, Some(&mut ps_overlay))?;
        }
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        unsafe { d3d.device.CreateSamplerState(&sampler_desc, Some(&mut sampler)) }?;
        Ok(Pipeline {
            vs: vs.ok_or_else(|| anyhow!("no vertex shader"))?,
            ps_mirror: ps_mirror.ok_or_else(|| anyhow!("no mirror shader"))?,
            ps_overlay: ps_overlay.ok_or_else(|| anyhow!("no overlay shader"))?,
            sampler: sampler.ok_or_else(|| anyhow!("no sampler"))?,
            crop_buffer: constant_buffer(&d3d.device, 16)?,
            overlay_buffer: constant_buffer(&d3d.device, 32)?,
        })
    }

    /// Draws `src` cropped to `uv` (`[u0, v0, du, dv]`) into the window
    /// rectangle `dst` of the current render target.
    pub fn draw_mirror(&self, ctx: &ID3D11DeviceContext, src: &SourceTexture, uv: [f32; 4], dst: Rect) {
        unsafe {
            ctx.UpdateSubresource(&self.crop_buffer, 0, None, uv.as_ptr() as *const _, 0, 0);
            self.bind(ctx, src, &self.ps_mirror, dst);
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None]));
        }
    }

    /// Draws the frozen `src` over the whole window, dimmed outside `selection`
    /// (frame pixels) with a border around it; no selection dims everything.
    pub fn draw_overlay(&self, ctx: &ID3D11DeviceContext, target: &SwapChainTarget, src: &SourceTexture, selection: Option<Rect>) {
        let (w, h) = (src.width as f32, src.height as f32);
        let sel = match selection {
            Some(r) => [r.x as f32 / w, r.y as f32 / h, (r.x + r.w) as f32 / w, (r.y + r.h) as f32 / h],
            None => [0.0; 4],
        };
        let has = if selection.is_some() { 1.0 } else { 0.0 };
        let params: [f32; 8] = [sel[0], sel[1], sel[2], sel[3], 1.0 / w, 1.0 / h, has, 0.0];
        let full = Rect { x: 0, y: 0, w: target.width as i32, h: target.height as i32 };
        unsafe {
            ctx.UpdateSubresource(&self.crop_buffer, 0, None, [0.0f32, 0.0, 1.0, 1.0].as_ptr() as *const _, 0, 0);
            ctx.UpdateSubresource(&self.overlay_buffer, 0, None, params.as_ptr() as *const _, 0, 0);
            self.bind(ctx, src, &self.ps_overlay, full);
            ctx.PSSetConstantBuffers(1, Some(&[Some(self.overlay_buffer.clone())]));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None]));
        }
    }

    unsafe fn bind(&self, ctx: &ID3D11DeviceContext, src: &SourceTexture, ps: &ID3D11PixelShader, dst: Rect) {
        unsafe {
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: dst.x as f32,
                TopLeftY: dst.y as f32,
                Width: dst.w as f32,
                Height: dst.h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.IASetInputLayout(None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.crop_buffer.clone())]));
            ctx.PSSetShader(ps, None);
            ctx.PSSetShaderResources(0, Some(&[Some(src.srv.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        }
    }
}

fn constant_buffer(device: &ID3D11Device, bytes: u32) -> anyhow::Result<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: bytes,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        ..Default::default()
    };
    let mut buffer = None;
    unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer)) }?;
    buffer.ok_or_else(|| anyhow!("no constant buffer"))
}

fn compile(entry: PCSTR, target: PCSTR) -> anyhow::Result<ID3DBlob> {
    let mut blob = None;
    let mut errors = None;
    let result = unsafe {
        D3DCompile(
            SHADER_SOURCE.as_ptr() as *const _,
            SHADER_SOURCE.len(),
            PCSTR::null(),
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(e) = result {
        let message = errors.map(|b| String::from_utf8_lossy(blob_bytes(&b)).into_owned()).unwrap_or_default();
        bail!("shader compilation failed: {e}\n{message}");
    }
    blob.ok_or_else(|| anyhow!("D3DCompile returned no bytecode"))
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}
