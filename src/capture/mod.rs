//! capture/
//! Everything on the PC side: the screen chosen through the portal (portal.rs), the pointer
//! for touchback (input.rs), and the GStreamer pipeline that captures the screen with the
//! sound and encodes it (encode.rs describes it, this file runs it).
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

mod encode;
mod input;
mod portal;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use gstreamer as gst;
use gstreamer::prelude::*;

use crate::cli::Stream;
pub use input::Input;
pub use portal::ScreenCast;

/// Set by `f` in the menu: the panel keeps showing the same image, marked "Freeze".
pub static FREEZE: AtomicBool = AtomicBool::new(false);
/// The image shown while frozen. Not per capture: a reconnection must not show the live screen.
static FROZEN_FRAME: Mutex<Option<gst::Buffer>> = Mutex::new(None);

/// Running capture pipeline, feeding appsinks. Dropping it stops the pipeline. The portal
/// session outlives it, so a new capture can start on the same screen without asking again.
pub struct Capture {
    pipeline: gst::Element,
    /// Frames dropped before the encoder since the start: the PC is too slow.
    pub dropped: Arc<AtomicU64>,
}

impl Capture {
    /// Capture `screen` and the PC sound. `audio_delay_ms` shifts the sound later to line it
    /// up with the image, or the image when negative.
    pub fn start(screen: &ScreenCast, stream: &Stream, audio_delay_ms: i64) -> anyhow::Result<Self> {
        gst::init()?;
        let description = encode::describe(screen, stream);
        tracing::debug!(%description, "pipeline");

        if let Some((element, package)) = missing_plugin() {
            bail!("GStreamer has no {element}: install {package} (Debian and Ubuntu name)");
        }
        let pipeline = gst::parse::launch(&description).context("cannot build the GStreamer pipeline")?;
        let bin = pipeline.downcast_ref::<gst::Bin>().context("pipeline is not a bin")?;
        if let Some(src) = bin.by_name("src") {
            restamp_on_arrival(&src, &pipeline)?;
            // The size PipeWire really sends: a whole multi-monitor workspace shows here.
            if let Some(pad) = src.static_pad("src") {
                pad.connect_notify(Some("caps"), |pad, _| {
                    if let Some(caps) = pad.current_caps() {
                        tracing::info!(%caps, "screen caps");
                    }
                });
            }
        }
        // The leaky queue before the encoder overflows when the PC cannot encode fast enough:
        // each overrun drops a frame.
        let dropped = Arc::new(AtomicU64::new(0));
        if let Some(queue) = bin.by_name("vq") {
            let dropped = Arc::clone(&dropped);
            queue.connect("overrun", false, move |_| {
                dropped.fetch_add(1, Ordering::Relaxed);
                None
            });
        }

        if let Some(overlay) = bin.by_name("freeze") {
            freeze_on_demand(&overlay)?;
        }

        pipeline.set_state(gst::State::Playing)?;
        tracing::info!("capture started");
        set_audio_delay(&pipeline, audio_delay_ms);
        Ok(Self { pipeline, dropped })
    }

    /// Element `name` of the pipeline: appsinks and encoder.
    pub fn element(&self, name: &str) -> Option<gst::Element> {
        self.pipeline.downcast_ref::<gst::Bin>()?.by_name(name)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // Timed: a stop that never ends shows in the log as a start without its end.
        let started = std::time::Instant::now();
        tracing::info!("capture stopping");
        let _ = self.pipeline.set_state(gst::State::Null);
        tracing::info!(ms = started.elapsed().as_millis() as u64, "capture stopped");
    }
}

/// Delay the sound by `ms`, or the screen image when `ms` is negative. Only at the start:
/// changed on a running pipeline, the same offsets froze the image in sccast (2026-10-22).
fn set_audio_delay(pipeline: &gst::Element, ms: i64) {
    let Some(bin) = pipeline.downcast_ref::<gst::Bin>() else { return };
    for (source, delay) in [("asrc", ms), ("src", -ms)] {
        if let Some(pad) = bin.by_name(source).and_then(|e| e.static_pad("src")) {
            pad.set_offset(delay.max(0).saturating_mul(1_000_000));
        }
    }
    // A receiver may play the sound as it arrives, whatever its date (a TV did, in sccast),
    // so dating it later is not enough: send each frame only when its date comes.
    if let Some(sink) = bin.by_name("asink").filter(|_| ms > 0) {
        sink.set_property("processing-deadline", 0u64);
        sink.set_property("sync", true);
    }
}

/// First element sbcast needs that GStreamer lacks, with its package. The .deb pulls them all
/// in; a build from source may miss one (x264enc is not in the default plugins).
fn missing_plugin() -> Option<(&'static str, &'static str)> {
    const NEEDED: [(&str, &str); 6] = [
        ("x264enc", "gstreamer1.0-plugins-ugly"),
        ("pipewiresrc", "gstreamer1.0-pipewire"),
        ("pulsesrc", "gstreamer1.0-plugins-good"),
        ("avenc_aac", "gstreamer1.0-libav"),
        ("appsink", "gstreamer1.0-plugins-base"),
        ("textoverlay", "gstreamer1.0-x"),
    ];
    NEEDED.into_iter().find(|(element, _)| gst::ElementFactory::find(element).is_none())
}

/// Ask `encoder` for a key frame with SPS and PPS.
/// Upstream on the src pad: not serialized, so no wait for the encoder's stream lock.
pub fn request_key_frame(encoder: &gst::Element) {
    let event = gst::Structure::builder("GstForceKeyUnit").field("all-headers", true).build();
    if let Some(pad) = encoder.static_pad("src") {
        pad.send_event(gst::event::CustomUpstream::new(event));
    }
}

/// Replace the timestamps of `src` buffers with the pipeline running time at arrival.
/// With pipewiresrc on KWin, even with do-timestamp, videorate produced 3 s of video
/// per wall-clock second: the source timestamps cannot be trusted.
fn restamp_on_arrival(src: &gst::Element, pipeline: &gst::Element) -> anyhow::Result<()> {
    let pad = src.static_pad("src").context("source has no src pad")?;
    let pipeline = pipeline.downgrade();
    pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
        let now = pipeline.upgrade().and_then(|p| p.current_running_time());
        if let Some(gst::PadProbeData::Buffer(buffer)) = &mut info.data {
            let buffer = buffer.make_mut();
            buffer.set_pts(now);
            buffer.set_dts(gst::ClockTime::NONE);
        }
        gst::PadProbeReturn::Ok
    });
    Ok(())
}

/// Repeat one frame while `FREEZE` is set, and show the overlay's text on it.
fn freeze_on_demand(overlay: &gst::Element) -> anyhow::Result<()> {
    let pad = overlay.static_pad("video_sink").context("overlay has no video pad")?;
    let overlay = overlay.downgrade();
    pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
        let frozen = FREEZE.load(Ordering::Relaxed);
        if let Some(gst::PadProbeData::Buffer(buffer)) = &mut info.data {
            freeze(frozen, &mut FROZEN_FRAME.lock().unwrap(), buffer);
        }
        if let Some(overlay) = overlay.upgrade() {
            overlay.set_property("silent", !frozen);
        }
        gst::PadProbeReturn::Ok
    });
    Ok(())
}

/// While `frozen`, replace `buffer` with the frame in `kept`, the first frozen one, keeping
/// the timestamps of `buffer`. A frame of another size cannot go to the encoder: it is kept instead.
fn freeze(frozen: bool, kept: &mut Option<gst::Buffer>, buffer: &mut gst::Buffer) {
    if !frozen {
        *kept = None;
        return;
    }
    match kept.as_ref().filter(|frame| frame.size() == buffer.size()) {
        Some(frame) => {
            // Shares the memory: the overlay copies it before writing, so `kept` stays clean.
            let mut frame = frame.copy();
            let stamped = frame.make_mut();
            stamped.set_pts(buffer.pts());
            stamped.set_dts(buffer.dts());
            stamped.set_duration(buffer.duration());
            *buffer = frame;
        }
        // A copy of its own: pool buffers go back to their pool.
        None => *kept = buffer.copy_deep().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_repeats_the_first_frozen_frame() {
        gst::init().unwrap();
        let frame = |byte: u8, pts: u64| {
            let mut buffer = gst::Buffer::from_slice(vec![byte; 6]);
            buffer.make_mut().set_pts(gst::ClockTime::from_mseconds(pts));
            buffer
        };
        let bytes = |buffer: &gst::Buffer| buffer.map_readable().unwrap().to_vec();
        let mut kept = None;

        let mut live = frame(1, 0);
        freeze(false, &mut kept, &mut live);
        assert_eq!((bytes(&live), kept.is_none()), (vec![1; 6], true));

        // The first frozen frame is kept, then shown with the next timestamps.
        let mut first = frame(2, 33);
        freeze(true, &mut kept, &mut first);
        assert_eq!(bytes(&first), vec![2; 6]);
        let mut next = frame(3, 66);
        freeze(true, &mut kept, &mut next);
        assert_eq!((bytes(&next), next.pts()), (vec![2; 6], Some(gst::ClockTime::from_mseconds(66))));

        // Another size replaces the kept frame; live again forgets it.
        let mut bigger = gst::Buffer::from_slice(vec![4u8; 8]);
        freeze(true, &mut kept, &mut bigger);
        assert_eq!(kept.as_ref().map(|frame| frame.size()), Some(8));
        freeze(false, &mut kept, &mut frame(5, 99));
        assert!(kept.is_none());
    }
}
