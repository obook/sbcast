//! eld.rs
//! AAC-ELD encoder for the RTSP panels. They play what the official client sends, AAC-ELD at
//! 44.1 kHz, stereo, 480 samples a frame, and only crackle on AAC-LC (room B22, 2026-11-10).
//! GStreamer's fdkaacenc does LC only, so this calls libfdk-aac itself, with the settings the
//! official client gives it (read from its binary and its capture).
//! The library is loaded when a cast starts, so the .deb only recommends it (libfdk-aac2) and a
//! PC without it casts without sound. So does a build without ELD: the setup fails.
//!
//! Author: O. Booklage
//! Date: November 2026
//! Licence: MIT

use std::ffi::{c_int, c_uint, c_void, CStr};
use std::ptr::{null, null_mut};

use anyhow::ensure;

const LIBRARY: &CStr = c"libfdk-aac.so.2";

/// Samples per channel in one frame, and its size in bytes once in 16-bit stereo.
pub const FRAME_SAMPLES: usize = 480;
pub const FRAME_BYTES: usize = FRAME_SAMPLES * 4;
/// The average of the official client's frames in its capture.
const BITRATE: c_uint = 192_000;
/// Room for the biggest frame the library can give.
const OUT_BYTES: usize = 8192;

// Parameters and identifiers of fdk-aac's aacenc_lib.h; the official client sets the same.
const AOT: c_uint = 0x100;
const PARAM_BITRATE: c_uint = 0x101;
const SAMPLE_RATE: c_uint = 0x103;
const GRANULE_LENGTH: c_uint = 0x105;
const CHANNEL_MODE: c_uint = 0x106;
const CHANNEL_ORDER: c_uint = 0x107;
const TRANSPORT: c_uint = 0x300;
const AOT_ER_AAC_ELD: c_uint = 39;
const MODE_STEREO: c_uint = 2;
const WAV_ORDER: c_uint = 1;
const TRANSPORT_RAW: c_uint = 0;
const IN_AUDIO_DATA: c_int = 0;
const OUT_BITSTREAM_DATA: c_int = 3;

#[repr(C)]
struct BufDesc {
    count: c_int,
    buffers: *mut *mut c_void,
    identifiers: *mut c_int,
    sizes: *mut c_int,
    element_sizes: *mut c_int,
}

#[repr(C)]
struct InArgs {
    samples: c_int,
    ancillary_bytes: c_int,
}

#[repr(C)]
struct OutArgs {
    bytes: c_int,
    samples: c_int,
    ancillary_bytes: c_int,
    bit_reservoir: c_int,
}

#[repr(C)]
struct Info {
    max_out_bytes: c_uint,
    max_ancillary_bytes: c_uint,
    in_fill_level: c_uint,
    channels: c_uint,
    frame_length: c_uint,
    delay: c_uint,
    delay_core: c_uint,
    config: [u8; 64],
    config_size: c_uint,
}

type Open = unsafe extern "C" fn(*mut *mut c_void, c_uint, c_uint) -> c_int;
type Close = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
type SetParam = unsafe extern "C" fn(*mut c_void, c_uint, c_uint) -> c_int;
type Encode = unsafe extern "C" fn(*mut c_void, *const BufDesc, *const BufDesc, *const InArgs, *mut OutArgs) -> c_int;
type GetInfo = unsafe extern "C" fn(*mut c_void, *mut Info) -> c_int;

/// The address of `name` in `library`, as the function type `T`.
fn symbol<T: Copy>(library: *mut c_void, name: &CStr) -> anyhow::Result<T> {
    // SAFETY: dlsym only reads the library's symbol table.
    let address = unsafe { libc::dlsym(library, name.as_ptr()) };
    ensure!(!address.is_null(), "libfdk-aac has no {name:?}");
    // SAFETY: `T` is the function pointer type aacenc_lib.h gives to this symbol.
    Ok(unsafe { std::mem::transmute_copy(&address) })
}

fn check(status: c_int, call: &str) -> anyhow::Result<()> {
    ensure!(status == 0, "libfdk-aac {call} failed (error {status:#x})");
    Ok(())
}

pub struct Encoder {
    handle: *mut c_void,
    close: Close,
    encode: Encode,
}

// SAFETY: the library's handle has no thread affinity, and an Encoder is used by one thread at a time.
unsafe impl Send for Encoder {}

impl Encoder {
    /// Load libfdk-aac and set it up for AAC-ELD, raw frames of `FRAME_SAMPLES` samples.
    pub fn open() -> anyhow::Result<Self> {
        // SAFETY: a plain dlopen; the library stays loaded for the whole run.
        let library = unsafe { libc::dlopen(LIBRARY.as_ptr(), libc::RTLD_NOW) };
        ensure!(!library.is_null(), "cannot load {LIBRARY:?}: install libfdk-aac2");
        let open: Open = symbol(library, c"aacEncOpen")?;
        let set_param: SetParam = symbol(library, c"aacEncoder_SetParam")?;
        let get_info: GetInfo = symbol(library, c"aacEncInfo")?;
        let mut encoder = Self { handle: null_mut(), close: symbol(library, c"aacEncClose")?, encode: symbol(library, c"aacEncEncode")? };

        // SAFETY: the signatures are those of aacenc_lib.h; the handle is the one aacEncOpen gave.
        unsafe {
            check(open(&mut encoder.handle, 0, 2), "open")?;
            for (param, value) in [
                (AOT, AOT_ER_AAC_ELD),
                (GRANULE_LENGTH, FRAME_SAMPLES as c_uint),
                (SAMPLE_RATE, 44_100),
                (CHANNEL_MODE, MODE_STEREO),
                (CHANNEL_ORDER, WAV_ORDER),
                (PARAM_BITRATE, BITRATE),
                (TRANSPORT, TRANSPORT_RAW),
            ] {
                check(set_param(encoder.handle, param, value), &format!("setting {param:#x}"))?;
            }
            // All null: applies the settings.
            check((encoder.encode)(encoder.handle, null(), null(), null(), null_mut()), "start")?;
            let mut info: Info = std::mem::zeroed();
            check(get_info(encoder.handle, &mut info), "info")?;
            ensure!(info.frame_length as usize == FRAME_SAMPLES, "libfdk-aac frames hold {} samples", info.frame_length);
        }
        Ok(encoder)
    }

    /// One frame of `FRAME_BYTES` bytes of 16-bit little-endian stereo, as one AAC-ELD frame.
    pub fn encode(&mut self, pcm: &[u8]) -> Option<Vec<u8>> {
        assert_eq!(pcm.len(), FRAME_BYTES);
        let mut out = [0u8; OUT_BYTES];
        // The library reads the input only; the cast is for the C signature.
        let mut input = pcm.as_ptr() as *mut c_void;
        let mut output = out.as_mut_ptr() as *mut c_void;
        let (mut in_id, mut in_size, mut in_element) = (IN_AUDIO_DATA, FRAME_BYTES as c_int, 2);
        let (mut out_id, mut out_size, mut out_element) = (OUT_BITSTREAM_DATA, OUT_BYTES as c_int, 1);
        let in_desc = BufDesc { count: 1, buffers: &mut input, identifiers: &mut in_id, sizes: &mut in_size, element_sizes: &mut in_element };
        let out_desc = BufDesc { count: 1, buffers: &mut output, identifiers: &mut out_id, sizes: &mut out_size, element_sizes: &mut out_element };
        let in_args = InArgs { samples: (FRAME_SAMPLES * 2) as c_int, ancillary_bytes: 0 };
        let mut out_args = OutArgs { bytes: 0, samples: 0, ancillary_bytes: 0, bit_reservoir: 0 };
        // SAFETY: every pointer is to a local that outlives the call, sized as the descriptors say.
        let status = unsafe { (self.encode)(self.handle, &in_desc, &out_desc, &in_args, &mut out_args) };
        (status == 0 && out_args.bytes > 0).then(|| out[..out_args.bytes as usize].to_vec())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: the handle comes from aacEncOpen and is closed once. When open() fails before
        // aacEncOpen it is still null, which aacEncClose accepts.
        unsafe { (self.close)(&mut self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_frame_for_each_480_samples() {
        // Only where libfdk-aac2 is installed.
        let Ok(mut encoder) = Encoder::open() else { return };
        let tone: Vec<u8> = (0..FRAME_SAMPLES * 20)
            .flat_map(|i| {
                let v = ((i as f32 * 0.0627).sin() * 9000.0) as i16;
                [v, v]
            })
            .flat_map(i16::to_le_bytes)
            .collect();
        let sizes: Vec<_> = tone.chunks(FRAME_BYTES).map(|frame| encoder.encode(frame).map(|au| au.len())).collect();
        assert!(sizes.iter().skip(2).all(|n| n.is_some_and(|n| (50..2000).contains(&n))), "{sizes:?}");
    }
}
