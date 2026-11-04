//! rtsp/
//! Cast the screen to an RTSP panel (the school's panels), with touch back. Read from the
//! traffic of the panels' Linux client and checked with a prototype on a panel (2026-11-02):
//!   8121   text: the panel's info, which holds its code, then the code back;
//!   57395  JSON lines: a heartbeat each second, the cast request (a person taps Accept);
//!   52030  the panel's states (STOP_MIRROR);
//!   51040  RTSP: DESCRIBE, SETUP video then sound, OPTIONS each second;
//!   video  the port the video SETUP gives: H.264 packets (wire.rs);
//!   52020  the touches, once the cast is accepted.
//! No sound yet.
//!
//! Author: O. Booklage
//! Date: November 2026
//! Licence: MIT

pub mod wire;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::capture::{request_key_frame, Capture, Input, ScreenCast};
use crate::device::CastDevice;
use crate::menu::{Settings, Status};
use crate::screenshare::approval::{Refused, APPROVAL_TIMEOUT};
use crate::screenshare::media::{self, send_frames, Framing, Link};
use crate::screenshare::pointer::{apply, Action, Button, TouchEvent};
use crate::session::Out;

const INFO_PORT: u16 = 8121;
const CONTROL_PORT: u16 = 57395;
const STATUS_PORT: u16 = 52030;
const RTSP_PORT: u16 = 51040;
const TOUCH_PORT: u16 = 52020;
const TIMEOUT: Duration = Duration::from_secs(5);
/// The panel's touches are on a 1920x1080 screen, whatever the video size (seen on a 4K panel).
const TOUCH_SCREEN: (f64, f64) = (1920.0, 1080.0);
/// Accepted on the panel; 2 means refused, or the dialog closed unanswered.
const ACCEPTED: i64 = 1;

/// Cast `screen` to the RTSP panel `device` until `stop` or the panel's end of the cast.
// ponytail: no reconnection after a loss, unlike screenshare/; add it if the school Wi-Fi cuts casts.
pub fn cast(device: &CastDevice, s: &Settings, screen: &ScreenCast, stop: &'static AtomicBool, out: &Out) -> anyhow::Result<()> {
    let ip = IpAddr::V4(device.ip);
    let name = &device.name;
    check_code(ip)?;
    let mut control = Control::open(ip, &s.name)?;
    if !control.wait_for_accept(name, stop, out)? {
        return Ok(());
    }
    let (panel_stopped, status) = watch_status(ip)?;
    let mut rtsp = Rtsp::open(ip)?;
    rtsp.request("DESCRIBE", &[])?;
    let (_, body) = rtsp.request("SETUP", &wire::video_setup(&s.name))?;
    let data_port = wire::data_port(&body).context("no video port in the panel's SETUP answer")?;
    rtsp.request("SETUP", &wire::audio_setup())?;

    // No sound yet: a sound branch nobody reads would only fill up. The video keeps the
    // screen's shape, as the official client does: with black bands, the touches landed off
    // toward the edges (a 16:10 PC, 2026-11-02).
    let resolution = screen.input().map_or(s.stream.resolution, |input| fit(input.size, s.stream.resolution));
    let stream = crate::cli::Stream { no_audio: true, resolution, ..s.stream.clone() };
    let capture = Capture::start(screen, &stream, 0)?;
    let encoder = capture.element("venc").context("no encoder in the pipeline")?;
    // ponytail: leaks one flag per cast (a byte): the threads want a 'static flag.
    let ended: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let link = Link::default();
    let video = media::connect(SocketAddr::new(ip, data_port), data_port)?;
    let sender = send_frames(&capture, "sink", video, Framing::Rtsp, stream.bitrate, &link, ended)?;
    show_stats(&capture, &link, out);
    if let Some(input) = screen.input() {
        follow_touches(ip, input, ended)?;
    }
    out.say(&format!("Casting to {name}"));

    let lost = loop {
        if stop.load(Ordering::SeqCst) || panel_stopped.load(Ordering::SeqCst) {
            break None;
        }
        if ended.load(Ordering::SeqCst) {
            break Some(link.lost().unwrap_or_else(|| "the video connection closed".into()));
        }
        std::thread::sleep(Duration::from_secs(1));
        if let Err(e) = control.beat() {
            break Some(format!("control: {e:#}"));
        }
        match rtsp.request("OPTIONS", &[]) {
            // The panel asks for a key frame when its decoder needs one.
            Ok((_, body)) if serde_json::from_slice::<Value>(&body).is_ok_and(|v| v["idr_req"] == "1") => request_key_frame(&encoder),
            Ok(_) => {}
            Err(e) => break Some(format!("RTSP: {e:#}")),
        }
    };
    ended.store(true, Ordering::SeqCst);
    let _ = sender.join();
    // Say goodbye: without it the panel keeps waiting about 15 s before its menu comes back.
    if lost.is_none() {
        let _ = rtsp.request("TEARDOWN", &[]);
    }
    let _ = status.shutdown(Shutdown::Both);
    match lost {
        Some(why) => bail!("lost {name}: {why}"),
        None => Ok(()),
    }
}

/// Ask the panel for its code, then give it back: the panel hands it to anyone who asks.
fn check_code(ip: IpAddr) -> anyhow::Result<()> {
    let info = ask(ip, &["getServerInfo", "1"])?;
    let pin = wire::pin(&info).with_context(|| format!("no code in the panel's info: {info}"))?;
    let answer = ask(ip, &["CHECKPASSWORD", &pin])?;
    if !answer.contains("success") {
        bail!(Refused::new(format!("the panel refused its own code: {}", answer.trim())));
    }
    Ok(())
}

/// One text request on port 8121; the panel answers and closes, or ends with an empty line.
fn ask(ip: IpAddr, lines: &[&str]) -> anyhow::Result<String> {
    let address = SocketAddr::new(ip, INFO_PORT);
    let mut socket = TcpStream::connect_timeout(&address, TIMEOUT).with_context(|| format!("cannot reach {address}"))?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    socket.write_all(&wire::text_request(lines))?;
    let mut answer = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let n = socket.read(&mut chunk).with_context(|| format!("no answer from {address}"))?;
        answer.extend_from_slice(&chunk[..n]);
        if n == 0 || answer.ends_with(b"\r\n\r\n") || answer.ends_with(b"}") {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// The JSON channel: one message per line, a heartbeat each second.
struct Control {
    writer: TcpStream,
    incoming: mpsc::Receiver<Value>,
    beats: u64,
    /// The panel said castState 1 before our cast: it already shows another sender.
    other_cast: bool,
}

/// castState 1 while someone casts; seen as "1" in the official client's log, a number accepted too.
// ponytail: only seen during our own cast (2026-11-02); untested with another sender on the panel.
fn casting(message: &Value) -> bool {
    let state = &message["castState"];
    state.as_i64() == Some(1) || state.as_str() == Some("1")
}

impl Control {
    fn open(ip: IpAddr, name: &str) -> anyhow::Result<Self> {
        let address = SocketAddr::new(ip, CONTROL_PORT);
        let socket = TcpStream::connect_timeout(&address, TIMEOUT).with_context(|| format!("cannot reach {address}"))?;
        let reader = BufReader::new(socket.try_clone()?);
        let (tx, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            for line in reader.lines().map_while(Result::ok) {
                match serde_json::from_str::<Value>(&line) {
                    Ok(message) => {
                        if tx.send(message).is_err() {
                            break;
                        }
                    }
                    Err(_) => tracing::debug!(%line, "RTSP control line not understood"),
                }
            }
        });
        let mut control = Self { writer: socket, incoming, beats: 0, other_cast: false };
        control.beat()?;
        let client = json!({"clientName": name, "clientType": 6, "isTouchable": 1, "reportClientInfo": "N",
            "versionCode": 23, "versionName": "v7.7.0721"});
        control.send(&client)?;
        Ok(control)
    }

    fn send(&mut self, message: &Value) -> anyhow::Result<()> {
        self.writer.write_all(format!("{message}\n").as_bytes()).context("cannot write to the panel")
    }

    fn beat(&mut self) -> anyhow::Result<()> {
        self.send(&json!({"heartbeat": self.beats}))?;
        self.beats += 1;
        // The panel answers each beat with its state.
        while let Ok(message) = self.incoming.try_recv() {
            self.note(&message);
        }
        Ok(())
    }

    fn note(&mut self, message: &Value) {
        tracing::debug!(%message, "RTSP control");
        self.other_cast |= casting(message);
    }

    /// Ask to cast and wait for the person at the panel. Ok(false) at `stop`.
    fn wait_for_accept(&mut self, name: &str, stop: &AtomicBool, out: &Out) -> anyhow::Result<bool> {
        self.send(&json!({"requestCast": 7}))?;
        let started = Instant::now();
        let mut said = None;
        while let Some(left) = APPROVAL_TIMEOUT.checked_sub(started.elapsed()) {
            if stop.load(Ordering::SeqCst) {
                return Ok(false);
            }
            // The menu counts down; the terminal says it once, and again if another cast shows up.
            let now = (left.as_secs(), self.other_cast);
            let news = said.map(|(_, other)| other) != Some(now.1);
            if news || matches!(out, Out::Menu(_)) && said != Some(now) {
                let other = if self.other_cast { format!(" {name} already shows another cast: accepting replaces it.") } else { String::new() };
                out.say(&format!("Tap Accept on the panel {name} ({} s)...{other}", left.as_secs()));
                said = Some(now);
            }
            match self.incoming.recv_timeout(Duration::from_secs(1)) {
                Ok(message) => match message["replyCastRequest"].as_i64() {
                    Some(ACCEPTED) => return Ok(true),
                    Some(_) => bail!(Refused::new("refused on the panel, or nobody tapped Accept in time")),
                    None => self.note(&message),
                },
                Err(mpsc::RecvTimeoutError::Timeout) => self.beat()?,
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("the panel closed the control channel"),
            }
        }
        bail!(Refused::new("nobody tapped Accept on the panel"))
    }
}

impl Drop for Control {
    /// Shut both ways: the reading thread's copy of the socket would keep it open, and the
    /// panel would wait for heartbeats that no longer come.
    fn drop(&mut self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

/// Follow the panel's states; the flag goes up when the panel ends the cast. The socket is
/// returned to be shut at the end: the reading thread holds a copy that would keep it open.
fn watch_status(ip: IpAddr) -> anyhow::Result<(Arc<AtomicBool>, TcpStream)> {
    let address = SocketAddr::new(ip, STATUS_PORT);
    let mut socket = TcpStream::connect_timeout(&address, TIMEOUT).with_context(|| format!("cannot reach {address}"))?;
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stopped);
    let owner = socket.try_clone()?;
    std::thread::spawn(move || {
        let mut chunk = [0; 1024];
        while let Ok(n @ 1..) = socket.read(&mut chunk) {
            let state = String::from_utf8_lossy(&chunk[..n]);
            tracing::info!(%state, "RTSP panel state");
            if state.contains("STOP_MIRROR") {
                flag.store(true, Ordering::SeqCst);
            }
        }
    });
    Ok((stopped, owner))
}

/// The RTSP connection, kept alive with an OPTIONS each second.
struct Rtsp {
    socket: TcpStream,
    host: String,
    cseq: u32,
}

impl Rtsp {
    fn open(ip: IpAddr) -> anyhow::Result<Self> {
        let address = SocketAddr::new(ip, RTSP_PORT);
        let socket = TcpStream::connect_timeout(&address, TIMEOUT).with_context(|| format!("cannot reach {address}"))?;
        socket.set_read_timeout(Some(TIMEOUT))?;
        Ok(Self { socket, host: ip.to_string(), cseq: 0 })
    }

    /// Send `method` and return the reply's header and body.
    fn request(&mut self, method: &str, body: &[u8]) -> anyhow::Result<(String, Vec<u8>)> {
        self.socket.write_all(&wire::rtsp_request(method, &self.host, self.cseq, body))?;
        self.cseq += 1;
        let mut reply = Vec::new();
        let mut chunk = [0; 4096];
        let end = loop {
            if let Some(at) = reply.windows(4).position(|w| w == b"\r\n\r\n") {
                break at;
            }
            let n = self.socket.read(&mut chunk).context("no RTSP answer")?;
            if n == 0 {
                bail!("the panel closed RTSP");
            }
            reply.extend_from_slice(&chunk[..n]);
        };
        let header = String::from_utf8_lossy(&reply[..end]).into_owned();
        if !header.starts_with("RTSP/1.0 200") {
            bail!("RTSP {method}: {}", header.lines().next().unwrap_or_default());
        }
        let mut body = reply.split_off(end + 4);
        let length = wire::content_length(&header);
        while body.len() < length {
            let n = self.socket.read(&mut chunk).context("RTSP body cut")?;
            if n == 0 {
                bail!("the panel closed RTSP");
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body.truncate(length);
        Ok((header, body))
    }
}

/// Pointer events from the panel's touches: bit 0 of the buttons is the finger.
#[derive(Default)]
struct Finger {
    down: bool,
}

impl Finger {
    fn event(&mut self, touch: wire::Touch) -> TouchEvent {
        let (x, y) = (f64::from(touch.x), f64::from(touch.y));
        // Wheel as in VNC: 8 up, 16 down. Unverified on a panel.
        if touch.buttons & 0x18 != 0 {
            let steps = if touch.buttons & 8 != 0 { -1.0 } else { 1.0 };
            return TouchEvent { action: Action::Wheel, x: 0.0, y: steps, point_x: 0.0, point_y: 0.0 };
        }
        let down = touch.buttons & 1 != 0;
        let action = match (self.down, down) {
            (false, true) => Action::Press(Button::Left),
            (true, false) => Action::Release(Button::Left),
            _ => Action::Move,
        };
        self.down = down;
        TouchEvent { action, x, y, point_x: 0.0, point_y: 0.0 }
    }
}

/// The largest size of `screen`'s shape inside `bounds`, in even pixels (H.264 wants them).
fn fit(screen: (f64, f64), bounds: (u32, u32)) -> (u32, u32) {
    let scale = (f64::from(bounds.0) / screen.0).min(f64::from(bounds.1) / screen.1);
    let even = |length: f64| ((length * scale / 2.0).round() as u32 * 2).max(2);
    (even(screen.0), even(screen.1))
}

/// A touch on the panel's 1920x1080 grid, moved onto the desktop of size `desktop`: the
/// video has the desktop's shape, so each axis scales on its own.
fn on_desktop(event: TouchEvent, desktop: (f64, f64)) -> TouchEvent {
    if event.action == Action::Wheel {
        return event;
    }
    let (x, y) = (event.x / TOUCH_SCREEN.0 * desktop.0, event.y / TOUCH_SCREEN.1 * desktop.1);
    TouchEvent { x, y, ..event }
}

/// `events` without the moves that another move follows at once: during a drag the panel
/// sends more positions than the desktop's pointer takes, and only the last one matters.
fn latest_moves(events: Vec<TouchEvent>) -> Vec<TouchEvent> {
    let mut kept: Vec<TouchEvent> = Vec::with_capacity(events.len());
    for event in events {
        if event.action == Action::Move && kept.last().is_some_and(|last| last.action == Action::Move) {
            kept.pop();
        }
        kept.push(event);
    }
    kept
}

/// Read the touches until `ended` and move the pointer. The panel sends them only on a
/// connection opened once the cast runs.
fn follow_touches(ip: IpAddr, input: Arc<Input>, ended: &'static AtomicBool) -> anyhow::Result<()> {
    let address = SocketAddr::new(ip, TOUCH_PORT);
    let mut socket = TcpStream::connect_timeout(&address, TIMEOUT).with_context(|| format!("cannot reach {address}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    std::thread::spawn(move || {
        let mut finger = Finger::default();
        let (mut pending, mut chunk) = (Vec::new(), [0; 4096]);
        while !ended.load(Ordering::SeqCst) {
            match socket.read(&mut chunk) {
                Ok(0) => {
                    tracing::info!("RTSP touches: the panel closed the connection");
                    break;
                }
                Ok(n) => pending.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(e) => {
                    tracing::info!("RTSP touches ended: {e}");
                    break;
                }
            }
            let whole = pending.len() / 8 * 8;
            let events: Vec<TouchEvent> = pending
                .drain(..whole)
                .collect::<Vec<u8>>()
                .chunks_exact(8)
                .inspect(|message| tracing::debug!("RTSP touch raw {message:02x?}"))
                .filter_map(|message| wire::touch(message.try_into().ok()?))
                .map(|touch| finger.event(touch))
                .collect();
            for event in latest_moves(events) {
                let (x, y, pressed) = (event.x, event.y, matches!(event.action, Action::Press(_)));
                let desktop = on_desktop(event, input.size);
                if pressed {
                    tracing::info!(x, y, desktop_x = desktop.x, desktop_y = desktop.y, "RTSP touch");
                }
                apply(&input, &desktop, input.size);
            }
        }
    });
    tracing::info!("RTSP touch back on");
    Ok(())
}

/// Live panel of the menu: bytes sent (bitrate), frames dropped.
fn show_stats(capture: &Capture, link: &Link, out: &Out) {
    let sent = Arc::clone(&link.sent);
    out.send(Status::Served(Box::new(move || sent.load(Ordering::Relaxed))));
    out.send(Status::Dropped(Arc::clone(&capture.dropped)));
    out.send(Status::NetworkDropped(Arc::clone(&link.dropped)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_touches_into_press_move_release() {
        let mut finger = Finger::default();
        let at = |buttons, x| wire::Touch { buttons, x, y: 455 };
        // A drag as captured: 01, 03 (still down), 00.
        assert_eq!(finger.event(at(1, 1485)).action, Action::Press(Button::Left));
        assert_eq!(finger.event(at(3, 1483)).action, Action::Move);
        let release = finger.event(at(0, 1483));
        assert_eq!((release.action, release.x, release.y), (Action::Release(Button::Left), 1483.0, 455.0));
        assert_eq!(finger.event(at(0, 1483)).action, Action::Move);
        assert_eq!(finger.event(at(16, 0)).y, 1.0);
    }

    #[test]
    fn keeps_the_screen_shape_and_maps_each_axis() {
        assert_eq!(fit((1536.0, 960.0), (1920, 1080)), (1728, 1080));
        assert_eq!(fit((1920.0, 1080.0), (1920, 1080)), (1920, 1080));
        assert_eq!(fit((1280.0, 1024.0), (1280, 720)), (900, 720));
        let touch = |x, y| TouchEvent { action: Action::Move, x, y, point_x: 0.0, point_y: 0.0 };
        let corner = on_desktop(touch(1920.0, 1080.0), (1536.0, 960.0));
        assert_eq!((corner.x, corner.y), (1536.0, 960.0));
        let left = on_desktop(touch(960.0, 540.0), (1536.0, 960.0));
        assert_eq!((left.x, left.y), (768.0, 480.0));
    }

    #[test]
    fn sees_another_cast_in_the_panel_state() {
        assert!(casting(&json!({"castState": "1", "multiScreen": "1"})));
        assert!(casting(&json!({"castState": 1})));
        assert!(!casting(&json!({"castState": "0"})));
        assert!(!casting(&json!({"heartbeat": 3})));
    }

    #[test]
    fn keeps_only_the_last_of_queued_moves() {
        let event = |action, x| TouchEvent { action, x, y: 0.0, point_x: 0.0, point_y: 0.0 };
        let press = event(Action::Press(Button::Left), 1.0);
        let events = vec![press, event(Action::Move, 2.0), event(Action::Move, 3.0), event(Action::Release(Button::Left), 3.0)];
        let kept: Vec<f64> = latest_moves(events).iter().map(|e| e.x).collect();
        assert_eq!(kept, [1.0, 3.0, 3.0]);
    }
}
