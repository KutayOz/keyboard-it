//! Small CoreMedia/VideoToolbox bridge. The decoder stays on the network worker;
//! immutable retained CVPixelBuffers cross to AppKit through a latest-frame slot.
use std::{ffi::c_void, ptr, time::Instant};
type Ref = *mut c_void;
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Time {
    value: i64,
    scale: i32,
    flags: u32,
    epoch: i64,
}
impl Time {
    fn micros(n: i64) -> Self {
        Self {
            value: n,
            scale: 1_000_000,
            flags: 1,
            epoch: 0,
        }
    }
}
#[repr(C)]
struct Timing {
    duration: Time,
    presentation: Time,
    decode: Time,
}
#[repr(C)]
struct Callback {
    callback: unsafe extern "C" fn(Ref, Ref, i32, u32, Ref, Time, Time),
    context: Ref,
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(value: Ref) -> Ref;
    fn CFRelease(value: Ref);
}
#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        a: Ref,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_length: i32,
        out: *mut Ref,
    ) -> i32;
    fn CMBlockBufferCreateWithMemoryBlock(
        a: Ref,
        block: Ref,
        len: usize,
        allocator: Ref,
        source: Ref,
        offset: usize,
        data_len: usize,
        flags: u32,
        out: *mut Ref,
    ) -> i32;
    fn CMBlockBufferReplaceDataBytes(data: *const u8, block: Ref, offset: usize, len: usize)
        -> i32;
    fn CMSampleBufferCreateReady(
        a: Ref,
        data: Ref,
        format: Ref,
        samples: isize,
        timing_count: isize,
        timing: *const Timing,
        size_count: isize,
        sizes: *const usize,
        out: *mut Ref,
    ) -> i32;
    fn CMVideoFormatDescriptionCreateForImageBuffer(a: Ref, image: Ref, out: *mut Ref) -> i32;
    fn CMSampleBufferCreateForImageBuffer(
        a: Ref,
        image: Ref,
        ready: u8,
        callback: Ref,
        context: Ref,
        format: Ref,
        timing: *const Timing,
        out: *mut Ref,
    ) -> i32;
    static kCMSampleAttachmentKey_DisplayImmediately: Ref;
    fn CMSampleBufferGetSampleAttachmentsArray(sample: Ref, create: u8) -> Ref;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFDictionarySetValue(dict: Ref, key: Ref, value: Ref);
    static kCFBooleanTrue: Ref;
}
#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTDecompressionSessionCreate(
        a: Ref,
        format: Ref,
        spec: Ref,
        attributes: Ref,
        callback: *const Callback,
        out: *mut Ref,
    ) -> i32;
    fn VTDecompressionSessionDecodeFrame(
        session: Ref,
        sample: Ref,
        flags: u32,
        context: Ref,
        info: *mut u32,
    ) -> i32;
    fn VTDecompressionSessionInvalidate(session: Ref);
    fn VTSessionSetProperty(session: Ref, key: Ref, value: Ref) -> i32;
    static kVTDecompressionPropertyKey_RealTime: Ref;
}
#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

pub struct Frame(Ref, pub Instant);
// A retained immutable CVPixelBuffer is safe to hand to the main-thread display.
unsafe impl Send for Frame {}
impl Drop for Frame {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}
impl Frame {
    pub fn sample(&self) -> Result<Sample, String> {
        unsafe {
            let mut format = ptr::null_mut();
            check(CMVideoFormatDescriptionCreateForImageBuffer(
                ptr::null_mut(),
                self.0,
                &mut format,
            ))?;
            let timing = Timing {
                duration: Time::default(),
                presentation: Time::micros(0),
                decode: Time::default(),
            };
            let mut sample = ptr::null_mut();
            let status = CMSampleBufferCreateForImageBuffer(
                ptr::null_mut(),
                self.0,
                1,
                ptr::null_mut(),
                ptr::null_mut(),
                format,
                &timing,
                &mut sample,
            );
            CFRelease(format);
            check(status)?;
            let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, 1);
            if !attachments.is_null() {
                CFDictionarySetValue(
                    CFArrayGetValueAtIndex(attachments, 0),
                    kCMSampleAttachmentKey_DisplayImmediately,
                    kCFBooleanTrue,
                );
            }
            Ok(Sample(sample.cast()))
        }
    }
}
// Preserve CoreMedia's Objective-C pointer encoding. A raw void pointer has
// the same ABI but objc2 correctly rejects it for enqueueSampleBuffer:.
#[repr(C)]
pub struct OpaqueSampleBuffer {
    _private: [u8; 0],
}
unsafe impl objc2::encode::RefEncode for OpaqueSampleBuffer {
    const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(
        &objc2::encode::Encoding::Struct("opaqueCMSampleBuffer", &[]),
    );
}
pub struct Sample(pub *mut OpaqueSampleBuffer);
impl Drop for Sample {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0.cast());
        }
    }
}
fn check(status: i32) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("VideoToolbox/CoreMedia: {status}"))
    }
}
pub struct Decoder {
    session: Ref,
    format: Ref,
    sps: Vec<u8>,
    pps: Vec<u8>,
    clock: Instant,
    output: Box<Option<Frame>>,
}
impl Default for Decoder {
    fn default() -> Self {
        Self {
            session: ptr::null_mut(),
            format: ptr::null_mut(),
            sps: vec![],
            pps: vec![],
            clock: Instant::now(),
            output: Box::new(None),
        }
    }
}
unsafe extern "C" fn decoded(
    context: Ref,
    _source: Ref,
    status: i32,
    _info: u32,
    image: Ref,
    _pts: Time,
    _duration: Time,
) {
    if status == 0 && !image.is_null() {
        *(context as *mut Option<Frame>) = Some(Frame(CFRetain(image), Instant::now()));
    }
}
impl Decoder {
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<Frame>, String> {
        unsafe {
            let units = annex_b(data)?;
            let mut changed = false;
            for n in &units {
                match n[0] & 31 {
                    7 if self.sps != *n => {
                        self.sps = n.to_vec();
                        changed = true;
                    }
                    8 if self.pps != *n => {
                        self.pps = n.to_vec();
                        changed = true;
                    }
                    _ => {}
                }
            }
            if changed {
                self.clear();
            }
            if self.session.is_null() {
                if self.sps.is_empty() || self.pps.is_empty() {
                    return Ok(None);
                }
                let pointers = [self.sps.as_ptr(), self.pps.as_ptr()];
                let sizes = [self.sps.len(), self.pps.len()];
                check(CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    ptr::null_mut(),
                    2,
                    pointers.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    &mut self.format,
                ))?;
                let cb = Callback {
                    callback: decoded,
                    context: (&mut *self.output as *mut Option<Frame>).cast(),
                };
                check(VTDecompressionSessionCreate(
                    ptr::null_mut(),
                    self.format,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &cb,
                    &mut self.session,
                ))?;
                let _ = VTSessionSetProperty(
                    self.session,
                    kVTDecompressionPropertyKey_RealTime,
                    kCFBooleanTrue,
                );
            }
            let mut avcc = Vec::with_capacity(data.len());
            for n in units {
                if !matches!(n[0] & 31, 7 | 8 | 9) {
                    avcc.extend((n.len() as u32).to_be_bytes());
                    avcc.extend(n);
                }
            }
            if avcc.is_empty() {
                return Ok(None);
            }
            let mut block = ptr::null_mut();
            check(CMBlockBufferCreateWithMemoryBlock(
                ptr::null_mut(),
                ptr::null_mut(),
                avcc.len(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                avcc.len(),
                0,
                &mut block,
            ))?;
            let copy = CMBlockBufferReplaceDataBytes(avcc.as_ptr(), block, 0, avcc.len());
            if copy != 0 {
                CFRelease(block);
                return Err(format!("CoreMedia copy: {copy}"));
            }
            let timing = Timing {
                duration: Time::default(),
                presentation: Time::micros(
                    self.clock.elapsed().as_micros().min(i64::MAX as u128) as i64
                ),
                decode: Time::default(),
            };
            let size = avcc.len();
            let mut sample = ptr::null_mut();
            let status = CMSampleBufferCreateReady(
                ptr::null_mut(),
                block,
                self.format,
                1,
                1,
                &timing,
                1,
                &size,
                &mut sample,
            );
            CFRelease(block);
            check(status)?;
            let mut info = 0;
            // Flags zero guarantees the callback completes before this function returns.
            let status = VTDecompressionSessionDecodeFrame(
                self.session,
                sample,
                0,
                ptr::null_mut(),
                &mut info,
            );
            CFRelease(sample);
            check(status)?;
            Ok(self.output.take())
        }
    }
    fn clear(&mut self) {
        unsafe {
            if !self.session.is_null() {
                VTDecompressionSessionInvalidate(self.session);
                CFRelease(self.session);
                self.session = ptr::null_mut();
            }
            if !self.format.is_null() {
                CFRelease(self.format);
                self.format = ptr::null_mut();
            }
            self.output.take();
        }
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        self.clear();
    }
}
fn annex_b(data: &[u8]) -> Result<Vec<&[u8]>, String> {
    let mut starts = vec![];
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i..].starts_with(&[0, 0, 0, 1]) {
            starts.push((i, 4));
            i += 4;
        } else if data[i..].starts_with(&[0, 0, 1]) {
            starts.push((i, 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    if starts.is_empty() {
        return Err("H.264 Annex-B start code missing".into());
    }
    let mut units = vec![];
    for (n, (offset, len)) in starts.iter().enumerate() {
        let end = starts.get(n + 1).map(|v| v.0).unwrap_or(data.len());
        let unit = &data[offset + len..end];
        if !unit.is_empty() {
            units.push(unit);
        }
    }
    Ok(units)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splits_three_and_four_byte_start_codes() {
        assert_eq!(
            annex_b(&[0, 0, 0, 1, 0x67, 2, 0, 0, 1, 0x68, 3]).unwrap(),
            vec![&[0x67, 2][..], &[0x68, 3][..]]
        );
        assert!(annex_b(&[1, 2, 3]).is_err());
    }
}

#[cfg(test)]
mod decode_tests {
    use super::*;
    #[test]
    fn real_h264_decodes_and_becomes_a_display_sample() {
        // Generated test pattern, 320x180, libx264, one IDR, no B-frames.
        let data = include_bytes!("../../tests/fixtures/desktop.h264");
        let mut decoder = Decoder::default();
        let frame = decoder
            .decode(data)
            .unwrap()
            .expect("VideoToolbox must produce an image");
        let sample = frame.sample().unwrap();
        assert!(!sample.0.is_null());
    }
}
