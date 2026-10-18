//! D3D11 desktop -> compute cursor/rotation/scale -> video processor NV12.
//! Only the compressed bitstream crosses system memory on this path.
use std::mem::ManuallyDrop;
use windows::{
    core::{s, Interface, Result},
    Win32::{
        Graphics::{
            Direct3D::{Fxc::*, D3D_SRV_DIMENSION_BUFFEREX},
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
        Media::MediaFoundation::*,
    },
};

pub struct Frame<'a> {
    pub texture: &'a ID3D11Texture2D,
    pub rotation: DXGI_MODE_ROTATION,
    pub width: u32,
    pub height: u32,
    pub pointer: &'a [u8],
    pub shape: &'a DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    pub pointer_x: i32,
    pub pointer_y: i32,
    pub pointer_visible: bool,
}

pub struct Converter {
    compositor: Compositor,
    video: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    input: ID3D11VideoProcessorInputView,
}
struct Compositor {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    shader: ID3D11ComputeShader,
    constants: ID3D11Buffer,
    cursor: Option<(ID3D11Buffer, ID3D11ShaderResourceView)>,
    last_cursor: Vec<u8>,
    sampler: ID3D11SamplerState,
    rgb: ID3D11UnorderedAccessView,
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}
impl Converter {
    pub fn new(device: &ID3D11Device, w: u32, h: u32, fps: u32) -> Result<Self> {
        unsafe {
            let context = device.GetImmediateContext()?;
            let video: ID3D11VideoDevice = device.cast()?;
            let video_context: ID3D11VideoContext = context.cast()?;
            let enumerator =
                video.CreateVideoProcessorEnumerator(&D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                    InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                    InputFrameRate: DXGI_RATIONAL {
                        Numerator: fps,
                        Denominator: 1,
                    },
                    InputWidth: w,
                    InputHeight: h,
                    OutputFrameRate: DXGI_RATIONAL {
                        Numerator: fps,
                        Denominator: 1,
                    },
                    OutputWidth: w,
                    OutputHeight: h,
                    Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                })?;
            if enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_R8G8B8A8_UNORM)?
                & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0 as u32
                == 0
                || enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12)?
                    & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32
                    == 0
            {
                return Err(windows::core::Error::new(
                    MF_E_UNSUPPORTED_D3D_TYPE,
                    "GPU NV12 conversion unavailable",
                ));
            }
            let processor = video.CreateVideoProcessor(&enumerator, 0)?;
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            // Full-range RGB -> limited-range BT.709. Match the encoder metadata.
            let rgb_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 1 << 4 };
            let yuv_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: (2 << 4) | (1 << 2),
            };
            video_context.VideoProcessorSetStreamColorSpace(&processor, 0, &rgb_space);
            video_context.VideoProcessorSetOutputColorSpace(&processor, &yuv_space);
            let compositor = Compositor::new(device, w, h)?;
            let mut input = None;
            video.CreateVideoProcessorInputView(
                &compositor.texture,
                &enumerator,
                &D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                },
                Some(&mut input),
            )?;
            Ok(Self {
                compositor,
                video,
                video_context,
                enumerator,
                processor,
                input: input.unwrap(),
            })
        }
    }
    pub fn convert(&mut self, frame: &Frame<'_>, sample: &IMFSample) -> Result<()> {
        unsafe {
            self.compositor.paint(frame)?;
            let buffer: IMFDXGIBuffer = sample.GetBufferByIndex(0)?.cast()?;
            let mut resource = None;
            buffer.GetResource(&ID3D11Texture2D::IID, &mut resource as *mut _ as *mut _)?;
            let texture: ID3D11Texture2D = resource.unwrap();
            let sub = buffer.GetSubresourceIndex()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let output_desc = if desc.ArraySize > 1 {
                D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2DARRAY,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2DArray: D3D11_TEX2D_ARRAY_VPOV {
                            MipSlice: sub % desc.MipLevels,
                            FirstArraySlice: sub / desc.MipLevels,
                            ArraySize: 1,
                        },
                    },
                }
            } else {
                D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPOV { MipSlice: sub },
                    },
                }
            };
            let mut output = None;
            self.video.CreateVideoProcessorOutputView(
                &texture,
                &self.enumerator,
                &output_desc,
                Some(&mut output),
            )?;
            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(self.input.clone())),
                ..Default::default()
            };
            let result = self.video_context.VideoProcessorBlt(
                &self.processor,
                output.as_ref().unwrap(),
                0,
                std::slice::from_ref(&stream),
            );
            ManuallyDrop::drop(&mut stream.pInputSurface);
            self.compositor.context.Flush();
            result
        }
    }
}

impl Compositor {
    fn new(device: &ID3D11Device, w: u32, h: u32) -> Result<Self> {
        unsafe {
            let context = device.GetImmediateContext()?;
            let code = compile_shader()?;
            let mut shader = None;
            device.CreateComputeShader(&code, None, Some(&mut shader))?;
            let mut constants = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 48,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )?;
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            let mut texture = None;
            device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_UNORDERED_ACCESS | D3D11_BIND_RENDER_TARGET).0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut texture),
            )?;
            let texture = texture.unwrap();
            let mut rgb = None;
            device.CreateUnorderedAccessView(&texture, None, Some(&mut rgb))?;
            Ok(Self {
                device: device.clone(),
                context,
                shader: shader.unwrap(),
                constants: constants.unwrap(),
                cursor: None,
                last_cursor: vec![],
                sampler: sampler.unwrap(),
                rgb: rgb.unwrap(),
                texture,
                width: w,
                height: h,
            })
        }
    }
    fn paint(&mut self, frame: &Frame<'_>) -> Result<()> {
        unsafe {
            if self.cursor.is_none() || self.last_cursor != frame.pointer {
                let mut bytes = frame.pointer.to_vec();
                bytes.resize(bytes.len().max(4).next_multiple_of(4), 0);
                let mut buffer = None;
                self.device.CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: bytes.len() as u32,
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        MiscFlags: D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: bytes.as_ptr().cast(),
                        ..Default::default()
                    }),
                    Some(&mut buffer),
                )?;
                let buffer = buffer.unwrap();
                let mut view = None;
                self.device.CreateShaderResourceView(
                    &buffer,
                    Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                        Format: DXGI_FORMAT_R32_TYPELESS,
                        ViewDimension: D3D_SRV_DIMENSION_BUFFEREX,
                        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                            BufferEx: D3D11_BUFFEREX_SRV {
                                FirstElement: 0,
                                NumElements: bytes.len() as u32 / 4,
                                Flags: D3D11_BUFFEREX_SRV_FLAG_RAW.0 as u32,
                            },
                        },
                    }),
                    Some(&mut view),
                )?;
                self.cursor = Some((buffer, view.unwrap()));
                self.last_cursor = frame.pointer.to_vec();
            }
            let constants = [
                frame.width,
                frame.height,
                self.width,
                self.height,
                frame.rotation.0 as u32,
                frame.pointer_x as u32,
                frame.pointer_y as u32,
                (frame.pointer_visible && !frame.pointer.is_empty()) as u32,
                frame.shape.Type,
                frame.shape.Width,
                if frame.shape.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
                    frame.shape.Height / 2
                } else {
                    frame.shape.Height
                },
                frame.shape.Pitch,
            ];
            self.context.UpdateSubresource(
                &self.constants,
                0,
                None,
                constants.as_ptr().cast(),
                0,
                0,
            );
            let mut source = None;
            self.device
                .CreateShaderResourceView(frame.texture, None, Some(&mut source))?;
            self.context.CSSetShader(&self.shader, None);
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            self.context.CSSetShaderResources(
                0,
                Some(&[source, Some(self.cursor.as_ref().unwrap().1.clone())]),
            );
            self.context
                .CSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.context.CSSetUnorderedAccessViews(
                0,
                1,
                Some([Some(self.rgb.clone())].as_ptr()),
                None,
            );
            self.context
                .Dispatch(self.width.div_ceil(8), self.height.div_ceil(8), 1);
            self.context
                .CSSetUnorderedAccessViews(0, 1, Some([None].as_ptr()), None);
            self.context.CSSetShaderResources(0, Some(&[None, None]));
            Ok(())
        }
    }
}

fn compile_shader() -> Result<Vec<u8>> {
    let source = include_bytes!("composite.hlsl");
    unsafe {
        let mut code = None;
        let mut errors = None;
        if let Err(e) = D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            s!("keyboard-it composite"),
            None,
            None,
            s!("main"),
            s!("cs_5_0"),
            D3DCOMPILE_ENABLE_STRICTNESS,
            0,
            &mut code,
            Some(&mut errors),
        ) {
            let message = errors
                .map(|b| {
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        b.GetBufferPointer().cast::<u8>(),
                        b.GetBufferSize(),
                    ))
                    .into_owned()
                })
                .unwrap_or_else(|| e.to_string());
            return Err(windows::core::Error::new(e.code(), message));
        }
        let code = code.unwrap();
        Ok(
            std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize())
                .to_vec(),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_compositing_shader_compiles_on_windows() {
        assert!(!super::compile_shader().unwrap().is_empty());
    }
    #[test]
    fn warp_compositor_preserves_rotation_and_cursor_operations() {
        use super::*;
        use windows::Win32::{Foundation::HMODULE, Graphics::Direct3D::D3D_DRIVER_TYPE_WARP};
        unsafe {
            let mut device = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
            .unwrap();
            let device = device.unwrap();
            let context = device.GetImmediateContext().unwrap();
            let mut pixels = vec![];
            for y in 0..8u8 {
                for x in 0..16u8 {
                    pixels.extend([33, y * 21, x * 13, 255]);
                }
            }
            let mut source = None;
            device
                .CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: 16,
                        Height: 8,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: pixels.as_ptr().cast(),
                        SysMemPitch: 64,
                        ..Default::default()
                    }),
                    Some(&mut source),
                )
                .unwrap();
            let source = source.unwrap();
            let shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            for (rotation, w, h, corners) in [
                (DXGI_MODE_ROTATION_IDENTITY, 16, 8, [(0, 0), (15, 7)]),
                (DXGI_MODE_ROTATION_ROTATE90, 8, 16, [(0, 7), (15, 0)]),
                (DXGI_MODE_ROTATION_ROTATE180, 16, 8, [(15, 7), (0, 0)]),
                (DXGI_MODE_ROTATION_ROTATE270, 8, 16, [(15, 0), (0, 7)]),
            ] {
                let mut c = Compositor::new(&device, w, h).unwrap();
                c.paint(&Frame {
                    texture: &source,
                    rotation,
                    width: w,
                    height: h,
                    pointer: &[],
                    shape: &shape,
                    pointer_x: 0,
                    pointer_y: 0,
                    pointer_visible: false,
                })
                .unwrap();
                let result = readback(&device, &context, &c.texture);
                for (i, (x, y)) in corners.into_iter().enumerate() {
                    let offset = if i == 0 { 0 } else { (w * h * 4 - 4) as usize };
                    assert_eq!(
                        &result[offset..offset + 4],
                        &[x * 13, y * 21, 33, 255],
                        "rotation {:?}",
                        rotation
                    );
                }
            }
            for (kind, bytes, expected) in [
                (2, vec![0, 0, 255, 255], [255, 0, 0, 255]), // opaque color
                (4, vec![3, 1, 255, 1], [255, 1, 34, 255]),  // masked XOR
                (4, vec![50, 100, 200, 0], [200, 100, 50, 255]), // masked replace
                (1, vec![0x80, 0x80], [255, 255, 222, 255]), // monochrome invert
                (1, vec![0, 0], [0, 0, 0, 255]),             // monochrome black
                (1, vec![0, 0x80], [255, 255, 255, 255]),    // monochrome white
                (1, vec![0x80, 0], [0, 0, 33, 255]),         // monochrome transparent
            ] {
                let shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO {
                    Type: kind,
                    Width: 1,
                    Height: if kind == 1 { 2 } else { 1 },
                    Pitch: if kind == 1 { 1 } else { 4 },
                    ..Default::default()
                };
                let mut c = Compositor::new(&device, 16, 8).unwrap();
                c.paint(&Frame {
                    texture: &source,
                    rotation: DXGI_MODE_ROTATION_IDENTITY,
                    width: 16,
                    height: 8,
                    pointer: &bytes,
                    shape: &shape,
                    pointer_x: 0,
                    pointer_y: 0,
                    pointer_visible: true,
                })
                .unwrap();
                let result = readback(&device, &context, &c.texture);
                assert_eq!(&result[..4], &expected, "cursor type {kind}, {bytes:?}");
                assert_eq!(&result[4..8], &[13, 0, 33, 255]);
            }
        }
    }
    unsafe fn readback(
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
        context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
        texture: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    ) -> Vec<u8> {
        use windows::Win32::Graphics::Direct3D11::*;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        device
            .CreateTexture2D(&desc, None, Some(&mut staging))
            .unwrap();
        let staging = staging.unwrap();
        context.CopyResource(&staging, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .unwrap();
        let mut output = vec![];
        for y in 0..desc.Height {
            output.extend_from_slice(std::slice::from_raw_parts(
                (mapped.pData as *const u8).add((y * mapped.RowPitch) as usize),
                (desc.Width * 4) as usize,
            ));
        }
        context.Unmap(&staging, 0);
        output
    }
}
