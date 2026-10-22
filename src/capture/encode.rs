//! encode.rs
//! Text description of the GStreamer pipeline (gst-launch syntax): screen, sound, encoders,
//! and the appsinks the panel connections read. mod.rs builds and runs it.
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

use super::portal::ScreenCast;
use crate::cli::Stream;

/// The whole pipeline for `stream`, from `screen`.
pub fn describe(screen: &ScreenCast, stream: &Stream) -> String {
    // `path` takes the node id; `target-object` wants the node serial, which the portal does not give.
    // PipeWire only sends frames on screen damage: keepalive resends the last one so a still
    // screen keeps producing frames, and videorate turns it into a constant frame rate.
    // Frames are stamped when pipewiresrc hands them out: the queue gives the encoding
    // its own thread, so a slow frame no longer holds the next ones in PipeWire (late
    // stamps put the sound ahead of the image). Leaky: drop frames rather than lag.
    let source = format!(
        "pipewiresrc name=src fd={} path={} keepalive-time=100 \
         ! queue name=vq leaky=downstream max-size-buffers=3 max-size-bytes=0 max-size-time=0 \
         ! videorate",
        screen.fd(),
        screen.node_id
    );
    let mut description = format!("{source} ! videoconvert ! videoscale ! {}", video_encode(stream));
    if !stream.no_audio {
        // Monitor of the default output: what the PC is playing. Works with pipewire-pulse.
        description.push_str(&format!(" pulsesrc name=asrc device=@DEFAULT_MONITOR@ ! {}", audio_encode()));
    }
    description
}

/// Raw format and encoder for `stream`.
/// Letterboxed to the output size whatever the screen shape; I420 is what profile High accepts.
/// Profile High with the encoder's auto level stays within 4.1 at 1080p30.
/// One access unit per buffer, SPS/PPS before each key frame (the panel can start on any).
/// Key frames every 5 s only: the panel asks for one when it needs it.
fn video_encode(stream: &Stream) -> String {
    let Stream { resolution: (width, height), fps, bitrate, .. } = stream;
    let key_interval = fps * 5;
    format!(
        "video/x-raw,format=I420,width={width},height={height},pixel-aspect-ratio=1/1,framerate={fps}/1 \
         ! x264enc name=venc tune=zerolatency speed-preset=veryfast key-int-max={key_interval} bitrate={bitrate} \
         ! video/x-h264,profile=high \
         ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au \
         ! appsink name=sink sync=false max-buffers=4"
    )
}

/// AAC at 44.1 kHz with ADTS headers, as the panel expects.
fn audio_encode() -> &'static str {
    "audioconvert ! audioresample ! audio/x-raw,rate=44100,channels=2 \
     ! avenc_aac bitrate=128000 ! aacparse ! audio/mpeg,stream-format=adts \
     ! appsink name=asink sync=false max-buffers=20"
}
