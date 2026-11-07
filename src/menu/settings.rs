//! settings.rs
//! Stream settings the menu edits: the values each row cycles through, the presets, and
//! the text shown for each row.
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

use crate::cli::Stream;

/// Settings the menu edits, prefilled from the command line.
#[derive(Clone)]
pub struct Settings {
    pub stream: Stream,
    pub audio_delay: i64,
    /// Code shown on the panel (`--pin`, or typed in the menu), kept for the next casts.
    pub pin: Option<String>,
    /// Name the panel shows when it asks to accept this PC (`--name`).
    pub name: String,
    /// Cast to an RTSP panel through its other receiver, on port 7385, which asks for the
    /// code shown on the panel: the school's panels run both.
    pub via_code: bool,
}

/// Name, resolution, bitrate.
pub(crate) const PRESETS: [(&str, (u32, u32), u32); 3] = [
    ("Default", (1920, 1080), 2500),
    ("Full HD", (1920, 1080), 4000),
    ("720p", (1280, 720), 2500),
];
const RESOLUTIONS: [(u32, u32); 3] = [(1920, 1080), (1280, 720), (854, 480)];
const BITRATES: [u32; 6] = [1500, 2500, 4000, 6000, 8000, 10000];
const FPS: [u32; 4] = [25, 30, 50, 60];
/// 50 ms steps, to tune lip sync by ear. Negative: the image waits.
const AUDIO_DELAY_STEP: i64 = 50;
const AUDIO_DELAY_RANGE: (i64, i64) = (-500, 1500);
/// Row of the audio delay, applied at the next cast.
pub(super) const AUDIO_DELAY_ROW: usize = 5;

pub(super) const ROWS: [&str; 7] = ["Preset", "Resolution", "Bitrate", "Frame rate", "Sound", "Audio delay", "Connection"];

/// Next (dir 1) or previous (dir -1) entry of `list`, wrapping. A value not in the list jumps to its first entry.
pub(super) fn cycle<T: PartialEq + Copy>(list: &[T], current: T, dir: isize) -> T {
    let len = list.len() as isize;
    let next = list.iter().position(|v| *v == current).map_or(0, |i| (i as isize + dir).rem_euclid(len));
    list[next as usize]
}

/// Index of the preset matching the current settings.
pub(crate) fn preset(s: &Settings) -> Option<usize> {
    PRESETS.iter().position(|(_, res, rate)| *res == s.stream.resolution && *rate == s.stream.bitrate)
}

/// Move the setting of `row` to its next (dir 1) or previous (dir -1) value.
pub(super) fn change(s: &mut Settings, row: usize, dir: isize) {
    let st = &mut s.stream;
    match row {
        0 => {
            let i = preset(s).map_or(0, |i| (i as isize + dir).rem_euclid(PRESETS.len() as isize) as usize);
            (_, s.stream.resolution, s.stream.bitrate) = PRESETS[i];
        }
        1 => st.resolution = cycle(&RESOLUTIONS, st.resolution, dir),
        2 => st.bitrate = cycle(&BITRATES, st.bitrate, dir),
        3 => st.fps = cycle(&FPS, st.fps, dir),
        4 => st.no_audio = !st.no_audio,
        AUDIO_DELAY_ROW => {
            let (min, max) = AUDIO_DELAY_RANGE;
            s.audio_delay = (s.audio_delay + AUDIO_DELAY_STEP * dir as i64).clamp(min, max);
        }
        _ => s.via_code = !s.via_code,
    }
}

/// Text shown for the setting of `row`.
pub(super) fn value(s: &Settings, row: usize) -> String {
    let st = &s.stream;
    match row {
        0 => preset(s).map_or("Custom", |i| PRESETS[i].0).to_string(),
        1 => format!("{}x{}", st.resolution.0, st.resolution.1),
        2 => format!("{} kbit/s", st.bitrate),
        3 => format!("{} fps", st.fps),
        4 => if st.no_audio { "off" } else { "on" }.to_string(),
        AUDIO_DELAY_ROW => format!("{} ms", s.audio_delay),
        _ => if s.via_code { "Code" } else { "Accept" }.to_string(),
    }
}
