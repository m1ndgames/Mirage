use anyhow::{anyhow, bail};
use windows::core::{s, Interface, PCSTR};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RenderTargetView,
    ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BUFFER_DESC, D3D11_FILTER_MIN_MAG_MIP_LINEAR,
    D3D11_SAMPLER_DESC, D3D11_TEXTURE2D_DESC, D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    IDXGISwapChain1, IDXGISwapChain2, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;

use crate::d3d::D3d;

const SHADER_SOURCE: &str = include_str!("shaders.hlsl");

/// Persistent copy of the newest captured frame, so the capture frame pool can
/// recycle its buffers while we keep drawing the last good image.
struct SourceTexture {
    texture: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    width: u32,
    height: u32,
}

pub struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap_chain: IDXGISwapChain2,
    frame_latency_waitable: HANDLE,
    rtv: ID3D11RenderTargetView,
    width: u32,
    height: u32,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    crop_buffer: ID3D11Buffer,
    source: Option<SourceTexture>,
}

impl Renderer {
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

        let vs_blob = compile(s!("vs_main"), s!("vs_5_0"))?;
        let ps_blob = compile(s!("ps_main"), s!("ps_5_0"))?;
        let mut vs = None;
        let mut ps = None;
        unsafe {
            d3d.device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vs))?;
            d3d.device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut ps))?;
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

        let buffer_desc = D3D11_BUFFER_DESC {
            ByteWidth: 16, // float2 + float2
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            ..Default::default()
        };
        let mut crop_buffer = None;
        unsafe { d3d.device.CreateBuffer(&buffer_desc, None, Some(&mut crop_buffer)) }?;

        Ok(Renderer {
            device: d3d.device.clone(),
            context: d3d.context.clone(),
            swap_chain,
            frame_latency_waitable,
            rtv: rtv.ok_or_else(|| anyhow!("no render target view"))?,
            width,
            height,
            vs: vs.ok_or_else(|| anyhow!("no vertex shader"))?,
            ps: ps.ok_or_else(|| anyhow!("no pixel shader"))?,
            sampler: sampler.ok_or_else(|| anyhow!("no sampler"))?,
            crop_buffer: crop_buffer.ok_or_else(|| anyhow!("no constant buffer"))?,
            source: None,
        })
    }

    /// Blocks until the swap chain can accept another frame (at most ~1 s).
    pub fn wait_for_frame_slot(&self) {
        unsafe {
            let _ = WaitForSingleObjectEx(self.frame_latency_waitable, 1000, false);
        }
    }

    /// (Re)creates the persistent source texture if the size changed.
    pub fn ensure_source(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        if self.source.as_ref().is_some_and(|s| s.width == width && s.height == height) {
            return Ok(());
        }
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
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut texture)) }?;
        let texture = texture.ok_or_else(|| anyhow!("no source texture"))?;
        let mut srv = None;
        unsafe { self.device.CreateShaderResourceView(&texture, None, Some(&mut srv)) }?;
        self.source = Some(SourceTexture {
            texture,
            srv: srv.ok_or_else(|| anyhow!("no shader resource view"))?,
            width,
            height,
        });
        println!("source texture {width}x{height}");
        Ok(())
    }

    /// GPU copy of a captured frame into the persistent texture. Sizes must match
    /// (`ensure_source` first).
    pub fn copy_frame(&self, frame: &ID3D11Texture2D) {
        if let Some(src) = &self.source {
            unsafe { self.context.CopyResource(&src.texture, frame) };
        }
    }

    /// Clears to black and, once a frame has been copied, draws the crop
    /// `uv = [u0, v0, du, dv]` stretched over the whole window.
    pub fn draw(&self, uv: [f32; 4]) -> anyhow::Result<()> {
        let ctx = &self.context;
        unsafe {
            ctx.ClearRenderTargetView(&self.rtv, &[0.0, 0.0, 0.0, 1.0]);
            let Some(src) = &self.source else { return Ok(()) };

            ctx.UpdateSubresource(&self.crop_buffer, 0, None, uv.as_ptr() as *const _, 0, 0);
            ctx.OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: self.width as f32,
                Height: self.height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.IASetInputLayout(None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.crop_buffer.clone())]));
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetShaderResources(0, Some(&[Some(src.srv.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.Draw(3, 0);
            // Unbind so the next CopyResource into the source texture has no hazard.
            ctx.PSSetShaderResources(0, Some(&[None]));
        }
        Ok(())
    }

    pub fn present(&self) -> anyhow::Result<()> {
        unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)) }.ok()?;
        Ok(())
    }
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
        let message = errors
            .map(|b| String::from_utf8_lossy(blob_bytes(&b)).into_owned())
            .unwrap_or_default();
        bail!("shader compilation failed: {e}\n{message}");
    }
    blob.ok_or_else(|| anyhow!("D3DCompile returned no bytecode"))
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}
