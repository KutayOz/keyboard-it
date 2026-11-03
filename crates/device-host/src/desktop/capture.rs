//! DXGI captures the current interactive primary output. No RDP session or driver.
use protocol::desktop::Display;
use windows::{
    core::{Interface, Result},
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
};

pub struct Capture {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    surface: Option<ID3D11Texture2D>,
    last: Vec<u8>,
    rotation: DXGI_MODE_ROTATION,
    pointer: Vec<u8>,
    shape: DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    pointer_x: i32,
    pointer_y: i32,
    pointer_visible: bool,
    pub display: Display,
}
impl Capture {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut found = None;
            for ai in 0..32 {
                let Ok(adapter) = factory.EnumAdapters1(ai) else {
                    break;
                };
                for oi in 0..32 {
                    let Ok(output) = adapter.EnumOutputs(oi) else {
                        break;
                    };
                    let desc = output.GetDesc()?;
                    if desc.AttachedToDesktop.as_bool()
                        && desc.DesktopCoordinates.left == 0
                        && desc.DesktopCoordinates.top == 0
                    {
                        found = Some((adapter.clone(), output, desc));
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            let (adapter, output, desc) = found.ok_or_else(|| {
                windows::core::Error::new(DXGI_ERROR_NOT_FOUND, "No active primary display")
            })?;
            let mut device = None;
            let mut context = None;
            let create = |flags,
                          device: &mut Option<ID3D11Device>,
                          context: &mut Option<ID3D11DeviceContext>| {
                D3D11CreateDevice(
                    &adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    flags,
                    None,
                    D3D11_SDK_VERSION,
                    Some(device),
                    None,
                    Some(context),
                )
            };
            create(
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                &mut device,
                &mut context,
            )
            .or_else(|_| create(D3D11_CREATE_DEVICE_BGRA_SUPPORT, &mut device, &mut context))?;
            let device = device.unwrap();
            let context = context.unwrap();
            let _ = context
                .cast::<ID3D11Multithread>()?
                .SetMultithreadProtected(true);
            let duplication = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?;
            let r = desc.DesktopCoordinates;
            let width = (r.right - r.left) as u32;
            let height = (r.bottom - r.top) as u32;
            Ok(Self {
                device,
                context,
                duplication,
                staging: None,
                surface: None,
                last: vec![],
                rotation: desc.Rotation,
                pointer: vec![],
                shape: Default::default(),
                pointer_x: 0,
                pointer_y: 0,
                pointer_visible: false,
                display: Display {
                    width,
                    height,
                    left: r.left,
                    top: r.top,
                    source_width: width,
                    source_height: height,
                },
            })
        }
    }
    /// Keep an owned GPU copy before releasing the DXGI frame. The encoder must
    /// never retain the desktop-duplication resource after ReleaseFrame.
    fn acquire(&mut self) -> Result<bool> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            match self
                .duplication
                .AcquireNextFrame(0, &mut info, &mut resource)
            {
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
                Err(e) => return Err(e),
                Ok(()) => {}
            }
            let result = (|| -> Result<bool> {
                if info.LastMouseUpdateTime != 0 {
                    self.pointer_x = info.PointerPosition.Position.x;
                    self.pointer_y = info.PointerPosition.Position.y;
                    self.pointer_visible = info.PointerPosition.Visible.as_bool();
                }
                if info.PointerShapeBufferSize > 0 && info.PointerShapeBufferSize <= 4 * 1024 * 1024
                {
                    self.pointer.resize(info.PointerShapeBufferSize as usize, 0);
                    let mut size = 0;
                    self.duplication.GetFramePointerShape(
                        info.PointerShapeBufferSize,
                        self.pointer.as_mut_ptr().cast(),
                        &mut size,
                        &mut self.shape,
                    )?;
                    self.pointer.truncate(size as usize);
                }
                // Mouse-only updates do not require another desktop texture copy.
                if info.LastPresentTime == 0 && self.surface.is_some() {
                    return Ok(false);
                }
                let texture: ID3D11Texture2D = resource
                    .as_ref()
                    .ok_or_else(|| {
                        windows::core::Error::new(
                            DXGI_ERROR_ACCESS_LOST,
                            "Desktop texture unavailable",
                        )
                    })?
                    .cast()?;
                if self.surface.is_none() {
                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    texture.GetDesc(&mut desc);
                    desc.Usage = D3D11_USAGE_DEFAULT;
                    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
                    desc.CPUAccessFlags = 0;
                    desc.MiscFlags = 0;
                    self.device
                        .CreateTexture2D(&desc, None, Some(&mut self.surface))?;
                }
                self.context
                    .CopyResource(self.surface.as_ref().unwrap(), &texture);
                Ok(true)
            })();
            let released = self.duplication.ReleaseFrame();
            let changed = result?;
            released?;
            Ok(changed)
        }
    }
    pub fn gpu_frame(&mut self) -> Result<Option<super::gpu::Frame<'_>>> {
        self.acquire()?;
        Ok(self.surface.as_ref().map(|texture| super::gpu::Frame {
            texture,
            rotation: self.rotation,
            width: self.display.width,
            height: self.display.height,
            pointer: &self.pointer,
            shape: &self.shape,
            pointer_x: self.pointer_x,
            pointer_y: self.pointer_y,
            pointer_visible: self.pointer_visible,
        }))
    }
    /// Compatibility fallback: oriented BGRA with the cursor composited.
    pub fn frame(&mut self) -> Result<Option<Vec<u8>>> {
        let changed = self.acquire()?;
        if !changed && !self.last.is_empty() {
            return Ok(Some(self.with_cursor()));
        }
        let Some(texture) = &self.surface else {
            return Ok(None);
        };
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if self.staging.is_none() {
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut self.staging))?;
            }
            let staging = self.staging.as_ref().unwrap();
            self.context.CopyResource(staging, texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let w = self.display.width as usize;
            let h = self.display.height as usize;
            self.last.resize(w * h * 4, 0);
            let pixels = std::slice::from_raw_parts(
                mapped.pData as *const u8,
                mapped.RowPitch as usize * desc.Height as usize,
            );
            for y in 0..h {
                // Normal displays use a row copy, not millions of tiny copies.
                if matches!(
                    self.rotation,
                    DXGI_MODE_ROTATION_IDENTITY | DXGI_MODE_ROTATION_UNSPECIFIED
                ) {
                    let src = y * mapped.RowPitch as usize;
                    self.last[y * w * 4..(y + 1) * w * 4]
                        .copy_from_slice(&pixels[src..src + w * 4]);
                    continue;
                }
                for x in 0..w {
                    let (sx, sy) = match self.rotation {
                        DXGI_MODE_ROTATION_ROTATE90 => (y, desc.Height as usize - 1 - x),
                        DXGI_MODE_ROTATION_ROTATE180 => {
                            (desc.Width as usize - 1 - x, desc.Height as usize - 1 - y)
                        }
                        DXGI_MODE_ROTATION_ROTATE270 => (desc.Width as usize - 1 - y, x),
                        _ => (x, y),
                    };
                    let src = sy * mapped.RowPitch as usize + sx * 4;
                    let dst = (y * w + x) * 4;
                    self.last[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
                }
            }
            self.context.Unmap(staging, 0);
            Ok(Some(self.with_cursor()))
        }
    }
    fn with_cursor(&self) -> Vec<u8> {
        let mut pixels = self.last.clone();
        if !self.pointer_visible || self.pointer.is_empty() {
            return pixels;
        }
        let sw = self.shape.Width as usize;
        let sh = if self.shape.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
            self.shape.Height as usize / 2
        } else {
            self.shape.Height as usize
        };
        let pitch = self.shape.Pitch as usize;
        let w = self.display.width as usize;
        let h = self.display.height as usize;
        for y in 0..sh {
            for x in 0..sw {
                let px = self.pointer_x + x as i32;
                let py = self.pointer_y + y as i32;
                if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
                    continue;
                }
                let dst = (py as usize * w + px as usize) * 4;
                if self.shape.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
                    let a = y * pitch + x / 8;
                    let b = (y + sh) * pitch + x / 8;
                    if b >= self.pointer.len() {
                        continue;
                    }
                    let mask = 0x80 >> (x % 8);
                    let and = if self.pointer[a] & mask != 0 { 255 } else { 0 };
                    let xor = if self.pointer[b] & mask != 0 { 255 } else { 0 };
                    for c in 0..3 {
                        pixels[dst + c] = (pixels[dst + c] & and) ^ xor;
                    }
                } else {
                    let src = y * pitch + x * 4;
                    if src + 4 > self.pointer.len() {
                        continue;
                    }
                    let alpha = self.pointer[src + 3] as u32;
                    for c in 0..3 {
                        pixels[dst + c] = if self.shape.Type
                            == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 as u32
                        {
                            if alpha != 0 {
                                pixels[dst + c] ^ self.pointer[src + c]
                            } else {
                                self.pointer[src + c]
                            }
                        } else {
                            ((self.pointer[src + c] as u32 * alpha
                                + pixels[dst + c] as u32 * (255 - alpha))
                                / 255) as u8
                        };
                    }
                }
            }
        }
        pixels
    }
}
