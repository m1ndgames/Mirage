use anyhow::{anyhow, Context};
use windows::core::Interface;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE,
};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;

pub struct D3d {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub factory: IDXGIFactory2,
    pub adapter_name: String,
}

/// Creates the one D3D11 device Mirage uses, on the hardware adapter whose
/// outputs include `source_handle`. DisplayLink and virtual-display adapters
/// never own the source monitor, so they are never picked; software adapters
/// are skipped outright. Falls back to the hardware adapter with the most
/// dedicated memory if no adapter claims the monitor.
pub fn create_for_monitor(source_handle: isize) -> anyhow::Result<D3d> {
    let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory1() }?;

    let mut owner: Option<(IDXGIAdapter1, String)> = None;
    let mut biggest: Option<(IDXGIAdapter1, String, usize)> = None;
    let mut index = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
        let desc = unsafe { adapter.GetDesc1() }?;
        let name_len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..name_len]);
        let software = desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0;

        let mut outputs = Vec::new();
        let mut out_index = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(out_index) } {
            outputs.push(unsafe { output.GetDesc() }?.Monitor.0 as isize);
            out_index += 1;
        }
        println!(
            "adapter {index}: {name}  software={software}  vram={} MiB  outputs={}",
            desc.DedicatedVideoMemory / (1024 * 1024),
            outputs.len()
        );

        if !software {
            if owner.is_none() && outputs.contains(&source_handle) {
                owner = Some((adapter.clone(), name.clone()));
            }
            if biggest.as_ref().map_or(true, |b| desc.DedicatedVideoMemory > b.2) {
                biggest = Some((adapter.clone(), name.clone(), desc.DedicatedVideoMemory));
            }
        }
        index += 1;
    }

    let (adapter, adapter_name) = match owner {
        Some(o) => o,
        None => {
            let b = biggest.ok_or_else(|| anyhow!("no hardware GPU found"))?;
            println!("no adapter owns the source monitor – falling back to {}", b.1);
            (b.0, b.1)
        }
    };

    let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .with_context(|| format!("D3D11CreateDevice on {adapter_name}"))?;

    Ok(D3d {
        device: device.ok_or_else(|| anyhow!("no device returned"))?,
        context: context.ok_or_else(|| anyhow!("no context returned"))?,
        factory,
        adapter_name,
    })
}

impl D3d {
    /// The same device wrapped for Windows.Graphics.Capture.
    pub fn winrt_device(&self) -> anyhow::Result<IDirect3DDevice> {
        let dxgi: IDXGIDevice = self.device.cast()?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }?;
        Ok(inspectable.cast()?)
    }
}
