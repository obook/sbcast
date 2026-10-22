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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Context;
use gstreamer as gst;
use gstreamer::prelude::*;

use crate::cli::Stream;
pub use input::Input;
pub use portal::ScreenCast;

/// Running capture pipeline, feeding appsinks. Dropping it stops the pipeline. The portal
/// session outlives it, so a new capture can start on the same screen without asking again.
pub struct Capture {
    pipeline: gst::Element,
    /// Frames dropped before the encoder since the start: the PC is too slow.
    pub dropped: Arc<AtomicU64>,
}

impl Capture {
    /// Capture `screen` and the PC sound. `audio_delay_ms` shifts the sound later to line it
    /// up with the image.
    pub fn start(screen: &ScreenCast, stream: &Stream, audio_delay_ms: u64) -> anyhow::Result<Self> {
        gst::init()?;
        let description = encode::describe(screen, stream);
        tracing::debug!(%description, "pipeline");

        let pipeline = gst::parse::launch(&description).context("cannot build pipeline (missing GStreamer plugin?)")?;
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

        pipeline.set_state(gst::State::Playing)?;
        tracing::info!("capture started");
        let capture = Self { pipeline, dropped };
        if let Some(pad) = capture.audio_pad() {
            set_audio_delay(&pad, audio_delay_ms);
        }
        Ok(capture)
    }

    /// Output of the sound source: the menu moves its offset to change the audio delay live.
    pub fn audio_pad(&self) -> Option<gst::Pad> {
        self.pipeline.downcast_ref::<gst::Bin>()?.by_name("asrc")?.static_pad("src")
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

/// Delay the sound by `ms`, on a running pipeline too (the menu changes it during a cast).
pub fn set_audio_delay(pad: &gst::Pad, ms: u64) {
    pad.set_offset(i64::try_from(ms).unwrap_or(0).saturating_mul(1_000_000));
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
