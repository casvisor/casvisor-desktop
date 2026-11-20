use anyhow::{anyhow, Context, Result};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::*;

use crate::protocol::Display;

pub struct Output {
    pub display: Display,
    adapter: IDXGIAdapter1,
    output: IDXGIOutput1,
}

pub fn outputs() -> Result<Vec<Output>> {
    let mut res = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let mut j = 0;
            while let Ok(output) = adapter.EnumOutputs(j) {
                j += 1;
                let desc = output.GetDesc()?;
                if !desc.AttachedToDesktop.as_bool() {
                    continue;
                }
                let Ok(output) = output.cast::<IDXGIOutput1>() else {
                    continue;
                };
                let rect = desc.DesktopCoordinates;
                let name_len = desc.DeviceName.iter().position(|&c| c == 0).unwrap_or(32);
                res.push(Output {
                    display: Display {
                        index: res.len() as u32,
                        name: String::from_utf16_lossy(&desc.DeviceName[..name_len]),
                        left: rect.left,
                        top: rect.top,
                        width: (rect.right - rect.left) as u32,
                        height: (rect.bottom - rect.top) as u32,
                        primary: rect.left == 0 && rect.top == 0,
                    },
                    adapter: adapter.clone(),
                    output,
                });
            }
        }
    }
    Ok(res)
}

pub enum Frame {
    Updated,
    Unchanged,
    AccessLost,
}

pub struct Duplicator {
    context: ID3D11DeviceContext,
    device: ID3D11Device,
    duplication: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    pub width: usize,
    pub height: usize,
}

impl Duplicator {
    pub fn new(output: &Output) -> Result<Self> {
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &output.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("failed to create the Direct3D device")?;
            let device = device.ok_or_else(|| anyhow!("no Direct3D device"))?;
            let context = context.ok_or_else(|| anyhow!("no Direct3D context"))?;

            let duplication = output.output.DuplicateOutput(&device)?;
            let desc = duplication.GetDesc();
            Ok(Duplicator {
                context,
                device,
                duplication,
                staging: None,
                width: desc.ModeDesc.Width as usize,
                height: desc.ModeDesc.Height as usize,
            })
        }
    }

    /// Waits up to `timeout_ms` for a new frame and hands its BGRA pixels and
    /// row pitch to `f`.
    pub fn acquire(&mut self, timeout_ms: u32, f: impl FnOnce(&[u8], usize)) -> Result<Frame> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            match self
                .duplication
                .AcquireNextFrame(timeout_ms, &mut info, &mut resource)
            {
                Ok(()) => {}
                Err(err) if err.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Frame::Unchanged),
                Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => return Ok(Frame::AccessLost),
                Err(err) => return Err(err.into()),
            }

            let result = self.copy(info.LastPresentTime != 0, resource, f);
            let _ = self.duplication.ReleaseFrame();
            result
        }
    }

    unsafe fn copy(
        &mut self,
        presented: bool,
        resource: Option<IDXGIResource>,
        f: impl FnOnce(&[u8], usize),
    ) -> Result<Frame> {
        if !presented {
            return Ok(Frame::Unchanged);
        }
        let texture: ID3D11Texture2D = resource
            .ok_or_else(|| anyhow!("no frame resource"))?
            .cast()?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(anyhow!("unsupported desktop format: {:?}", desc.Format));
        }
        if self.staging.is_none() {
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            let mut staging = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            self.staging = staging;
        }
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| anyhow!("no staging texture"))?;

        self.context.CopyResource(staging, &texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        self.context
            .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let pitch = mapped.RowPitch as usize;
        let data = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * self.height);
        f(data, pitch);
        self.context.Unmap(staging, 0);
        Ok(Frame::Updated)
    }
}
