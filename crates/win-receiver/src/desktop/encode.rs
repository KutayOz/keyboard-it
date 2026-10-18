//! Native Media Foundation H.264. Async hardware transforms are driven by their
//! event queue; software is a fallback, with bounded, low-latency output draining.
use protocol::stream::StreamSettings;
use std::{mem::ManuallyDrop, time::Instant};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4};
use windows::{
    core::{Interface, Result},
    Win32::{
        Media::MediaFoundation::*,
        System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED},
    },
};

pub struct Runtime;
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}
pub struct Encoder {
    transform: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    clock: Instant,
    pub fps: u32,
    pub bitrate_kbps: u32,
    gpu: Option<Gpu>,
    pub width: u32,
    pub height: u32,
    pub hardware: bool,
    input_credit: u32,
    pub skipped: u32,
}
struct Gpu {
    converter: super::gpu::Converter,
    allocator: IMFVideoSampleAllocatorEx,
    _manager: IMFDXGIDeviceManager,
}
impl Encoder {
    pub fn initialize() -> Result<Runtime> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                CoUninitialize();
                return Err(e);
            }
        }
        Ok(Runtime)
    }
    pub fn new(
        w: u32,
        h: u32,
        settings: StreamSettings,
        device: Option<&ID3D11Device>,
        allow_hardware: bool,
    ) -> Result<Self> {
        let mut size = settings.dimensions(w, h);
        let mut last =
            windows::core::Error::new(MF_E_TOPO_CODEC_NOT_FOUND, "No supported H.264 encoder");
        let mut rates = vec![settings.fps, settings.fps.min(60), settings.fps.min(30)];
        rates.dedup();
        loop {
            // Prefer the source dimensions at a lower supported FPS over
            // silently reducing resolution to obtain the requested frame rate.
            for fps in &rates {
                let settings = StreamSettings {
                    fps: *fps,
                    ..settings
                };
                for (hardware, gpu_device) in [(true, device), (true, None), (false, None)] {
                    if hardware && !allow_hardware {
                        continue;
                    }
                    match Self::try_new(size.0, size.1, settings, hardware, gpu_device) {
                        Ok(e) => return Ok(e),
                        Err(e) => last = e,
                    }
                }
            }
            if size.0 <= 1280 && size.1 <= 720 {
                return Err(last);
            }
            size = ((size.0 * 3 / 4 & !1).max(2), (size.1 * 3 / 4 & !1).max(2));
        }
    }
    fn try_new(
        w: u32,
        h: u32,
        settings: StreamSettings,
        hardware: bool,
        device: Option<&ID3D11Device>,
    ) -> Result<Self> {
        unsafe {
            let input = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_NV12,
            };
            let output = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_H264,
            };
            let mut list = std::ptr::null_mut();
            let mut count = 0;
            let flags = if hardware {
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
            } else {
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER | MFT_ENUM_FLAG_LOCALMFT
            };
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                flags,
                Some(&input),
                Some(&output),
                &mut list,
                &mut count,
            )?;
            let activators = if count == 0 {
                vec![]
            } else {
                std::slice::from_raw_parts(list, count as usize).to_vec()
            };
            if !list.is_null() {
                for slot in std::slice::from_raw_parts_mut(list, count as usize) {
                    *slot = None;
                }
                CoTaskMemFree(Some(list.cast()));
            }
            let mut last =
                windows::core::Error::new(MF_E_TOPO_CODEC_NOT_FOUND, "No supported H.264 encoder");
            for activate in activators.into_iter().flatten() {
                let attempt = (|| -> Result<Self> {
                    let t: IMFTransform = activate.ActivateObject()?;
                    let attrs = t.GetAttributes().ok();
                    let asynchronous = attrs
                        .as_ref()
                        .and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok())
                        .unwrap_or(0)
                        != 0;
                    if asynchronous {
                        attrs
                            .as_ref()
                            .unwrap()
                            .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                    }
                    if let Some(a) = &attrs {
                        let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
                    }
                    let manager = if let Some(device) = device {
                        if attrs
                            .as_ref()
                            .and_then(|a| a.GetUINT32(&MF_SA_D3D11_AWARE).ok())
                            .unwrap_or(0)
                            == 0
                        {
                            return Err(windows::core::Error::new(
                                MF_E_UNSUPPORTED_D3D_TYPE,
                                "Encoder does not accept GPU surfaces",
                            ));
                        }
                        let mut token = 0;
                        let mut manager = None;
                        MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
                        let manager = manager.unwrap();
                        manager.ResetDevice(device, token)?;
                        t.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;
                        Some(manager)
                    } else {
                        None
                    };
                    if let Ok(codec) = t.cast::<ICodecAPI>() {
                        let _ = codec.SetValue(&CODECAPI_AVLowLatencyMode, &boolean(true));
                        let _ = codec.SetValue(&CODECAPI_AVEncCommonRealTime, &boolean(true));
                        let _ = codec.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &integer(0));
                        let _ = codec.SetValue(&CODECAPI_AVEncMPVGOPSize, &integer(settings.fps));
                        let _ = codec.SetValue(&CODECAPI_AVEncCommonRateControlMode, &integer(0)); // CBR
                        let _ = codec.SetValue(
                            &CODECAPI_AVEncCommonMeanBitRate,
                            &integer(settings.bitrate(w, h) * 1000),
                        );
                    }
                    let out =
                        media_type(w, h, settings.fps, &MFVideoFormat_H264, device.is_some())?;
                    out.SetUINT32(&MF_MT_AVG_BITRATE, settings.bitrate(w, h) * 1000)?;
                    out.SetUINT32(&MF_MT_MPEG2_PROFILE, 100)?;
                    t.SetOutputType(0, &out, 0)?;
                    let input_type =
                        media_type(w, h, settings.fps, &MFVideoFormat_NV12, device.is_some())?;
                    t.SetInputType(0, &input_type, 0)?;
                    let gpu = if let (Some(device), Some(manager)) = (device, manager) {
                        let mut raw = std::ptr::null_mut();
                        MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocatorEx::IID, &mut raw)?;
                        let allocator = IMFVideoSampleAllocatorEx::from_raw(raw);
                        allocator.SetDirectXManager(&manager)?;
                        let mut attrs = None;
                        MFCreateAttributes(&mut attrs, 2)?;
                        let attrs = attrs.unwrap();
                        attrs
                            .SetUINT32(&MF_SA_D3D11_BINDFLAGS, D3D11_BIND_RENDER_TARGET.0 as u32)?;
                        attrs.SetUINT32(&MF_SA_D3D11_USAGE, D3D11_USAGE_DEFAULT.0 as u32)?;
                        // Samples return to this bounded pool only after the MFT releases
                        // them; in-flight GPU surfaces are never overwritten.
                        allocator.InitializeSampleAllocatorEx(2, 4, &attrs, &input_type)?;
                        Some(Gpu {
                            converter: super::gpu::Converter::new(device, w, h, settings.fps)?,
                            allocator,
                            _manager: manager,
                        })
                    } else {
                        None
                    };
                    t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                    t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
                    let events = if asynchronous { Some(t.cast()?) } else { None };
                    Ok(Self {
                        transform: t,
                        events,
                        clock: Instant::now(),
                        fps: settings.fps,
                        bitrate_kbps: settings.bitrate(w, h),
                        gpu,
                        width: w,
                        height: h,
                        hardware,
                        input_credit: 0,
                        skipped: 0,
                    })
                })();
                match attempt {
                    Ok(e) => return Ok(e),
                    Err(e) => {
                        last = e;
                        let _ = activate.ShutdownObject();
                    }
                }
            }
            Err(last)
        }
    }
    pub fn force_keyframe(&self) {
        unsafe {
            if let Ok(c) = self.transform.cast::<ICodecAPI>() {
                let _ = c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &integer(1));
            }
        }
    }
    pub fn encode(&mut self, bgra: &[u8], sw: u32, sh: u32) -> Result<Vec<Vec<u8>>> {
        unsafe {
            let data = nv12(bgra, sw, sh, self.width, self.height);
            let buffer = MFCreateMemoryBuffer(data.len() as u32)?;
            let mut p = std::ptr::null_mut();
            buffer.Lock(&mut p, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            self.submit(&sample)
        }
    }
    pub fn uses_gpu_surfaces(&self) -> bool {
        self.gpu.is_some()
    }
    pub fn encode_gpu(&mut self, frame: &super::gpu::Frame<'_>) -> Result<Vec<Vec<u8>>> {
        // Drain completion events even when no surface is available. Allocation
        // pressure discards a capture, never an encoded reference frame.
        let mut output = self.poll_available()?;
        if self.events.is_some() && self.input_credit == 0 {
            self.skipped = self.skipped.saturating_add(1);
            return Ok(output);
        }
        let Some(gpu) = &mut self.gpu else {
            unreachable!()
        };
        let sample = match unsafe { gpu.allocator.AllocateSample() } {
            Ok(sample) => sample,
            Err(e) if e.code() == MF_E_SAMPLEALLOCATOR_EMPTY => {
                self.skipped = self.skipped.saturating_add(1);
                return Ok(output);
            }
            Err(e) => return Err(e),
        };
        gpu.converter.convert(frame, &sample)?;
        output.extend(unsafe { self.submit(&sample)? });
        Ok(output)
    }
    fn poll_available(&mut self) -> Result<Vec<Vec<u8>>> {
        let mut output = vec![];
        if self.events.is_some() {
            for _ in 0..64 {
                if !unsafe { self.poll_event(&mut output)? } {
                    break;
                }
            }
        }
        Ok(output)
    }
    unsafe fn submit(&mut self, sample: &IMFSample) -> Result<Vec<Vec<u8>>> {
        let mut output = self.poll_available()?;
        if self.events.is_some() && self.input_credit == 0 {
            self.skipped = self.skipped.saturating_add(1);
            return Ok(output);
        }
        sample
            .SetSampleTime((self.clock.elapsed().as_nanos() / 100).min(i64::MAX as u128) as i64)?;
        sample.SetSampleDuration(10_000_000 / self.fps as i64)?;
        match self.transform.ProcessInput(0, sample, 0) {
            Err(e) if e.code() == MF_E_NOTACCEPTING && self.events.is_none() => {
                for _ in 0..16 {
                    if let Some(bytes) = self.drain()? {
                        output.push(bytes);
                    } else {
                        break;
                    }
                }
                self.transform.ProcessInput(0, sample, 0)?;
            }
            other => other?,
        }
        if self.events.is_some() {
            self.input_credit -= 1;
            output.extend(self.poll_available()?);
        } else {
            for _ in 0..16 {
                if let Some(bytes) = self.drain()? {
                    output.push(bytes);
                } else {
                    break;
                }
            }
        }
        Ok(output)
    }
    unsafe fn poll_event(&mut self, output: &mut Vec<Vec<u8>>) -> Result<bool> {
        match self
            .events
            .as_ref()
            .unwrap()
            .GetEvent(MF_EVENT_FLAG_NO_WAIT)
        {
            Ok(event) => {
                event.GetStatus()?.ok()?;
                match event.GetType()? {
                    v if v == METransformNeedInput.0 as u32 => {
                        self.input_credit = self.input_credit.saturating_add(1)
                    }
                    v if v == METransformHaveOutput.0 as u32 => {
                        if let Some(bytes) = self.drain()? {
                            output.push(bytes);
                        }
                    }
                    _ => {}
                }
                Ok(true)
            }
            Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => Ok(false),
            Err(e) => Err(e),
        }
    }

    unsafe fn drain(&self) -> Result<Option<Vec<u8>>> {
        let info = self.transform.GetOutputStreamInfo(0)?;
        let provided = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
        let sample = if provided {
            None
        } else {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(
                info.cbSize.max(self.width * self.height * 4),
            )?)?;
            Some(s)
        };
        let mut out = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(sample),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0;
        let result = self.transform.ProcessOutput(0, &mut out, &mut status);
        let sample = ManuallyDrop::take(&mut out[0].pSample);
        let _events = ManuallyDrop::take(&mut out[0].pEvents);
        if let Err(e) = result {
            if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                return Ok(None);
            }
            return Err(e);
        }
        let Some(sample) = sample else {
            return Ok(None);
        };
        let buffer = sample.ConvertToContiguousBuffer()?;
        let mut p = std::ptr::null_mut();
        let mut len = 0;
        buffer.Lock(&mut p, None, Some(&mut len))?;
        let mut bytes = std::slice::from_raw_parts(p, len as usize).to_vec();
        buffer.Unlock()?;
        // Some MFTs expose SPS/PPS in the output media type rather than repeating
        // them in the first IDR. Prepend that header for self-contained decoding.
        if sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0 {
            if let Ok(t) = self.transform.GetOutputCurrentType(0) {
                if let Ok(n) = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) {
                    if n > 0 && n < 65536 {
                        let mut header = vec![0; n as usize];
                        if t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut header, None)
                            .is_ok()
                            && (header.starts_with(&[0, 0, 1]) || header.starts_with(&[0, 0, 0, 1]))
                        {
                            header.extend(bytes);
                            bytes = header;
                        }
                    }
                }
            }
        }
        Ok(Some(bytes))
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}
unsafe fn media_type(
    w: u32,
    h: u32,
    fps: u32,
    subtype: &windows::core::GUID,
    gpu: bool,
) -> Result<IMFMediaType> {
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
    t.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)?;
    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
    t.SetUINT32(&MF_MT_YUV_MATRIX, if gpu { 1 } else { 2 })?;
    t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, 2)?;
    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    Ok(t)
}
fn nv12(src: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0; (w * h * 3 / 2) as usize];
    let rgb = |x: u32, y: u32| {
        let p = ((y * sh / h * sw + x * sw / w) * 4) as usize;
        (src[p + 2] as i32, src[p + 1] as i32, src[p] as i32)
    };
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = rgb(x, y);
            out[(y * w + x) as usize] =
                (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
        }
    }
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut r = 0;
            let mut g = 0;
            let mut b = 0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (rr, gg, bb) = rgb(x + dx, y + dy);
                    r += rr;
                    g += gg;
                    b += bb;
                }
            }
            r /= 4;
            g /= 4;
            b /= 4;
            let p = (w * h + y / 2 * w + x) as usize;
            out[p] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            out[p + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
    out
}

fn boolean(value: bool) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        (*v.Anonymous.Anonymous).vt = VT_BOOL;
        (*v.Anonymous.Anonymous).Anonymous.boolVal = value.into();
    }
    v
}
fn integer(value: u32) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        (*v.Anonymous.Anonymous).vt = VT_UI4;
        (*v.Anonymous.Anonymous).Anonymous.ulVal = value;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nv12_black_and_white_have_video_range_luma_and_neutral_chroma() {
        let black = nv12(&[0, 0, 0, 255].repeat(4), 2, 2, 2, 2);
        assert_eq!(black, vec![16, 16, 16, 16, 128, 128]);
        let white = nv12(&[255, 255, 255, 255].repeat(4), 2, 2, 2, 2);
        assert_eq!(white, vec![235, 235, 235, 235, 128, 128]);
    }
    #[test]
    fn native_software_encoder_emits_decodable_h264_access_units() {
        let _runtime = Encoder::initialize().unwrap();
        let mut encoder =
            Encoder::try_new(320, 180, StreamSettings::default(), false, None).unwrap();
        encoder.force_keyframe();
        let pixels = [32, 64, 128, 255].repeat(320 * 180);
        let mut encoded = vec![];
        for _ in 0..10 {
            encoded.extend(encoder.encode(&pixels, 320, 180).unwrap());
            if !encoded.is_empty() {
                break;
            }
        }
        assert!(
            !encoded.is_empty(),
            "software MFT must emit a low-latency frame"
        );
        let frame = &encoded[0];
        assert!(
            frame.starts_with(&[0, 0, 1]) || frame.starts_with(&[0, 0, 0, 1]),
            "Media Foundation must emit Annex-B H.264"
        );
        assert!(
            frame
                .windows(5)
                .any(|p| p[..4] == [0, 0, 0, 1] && p[4] & 31 == 7)
                || frame
                    .windows(4)
                    .any(|p| p[..3] == [0, 0, 1] && p[3] & 31 == 7),
            "first frame must contain SPS"
        );
    }
}
