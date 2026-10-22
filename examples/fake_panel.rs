//! fake_panel.rs
//! A stand-in for an interactive panel, to try sbcast on one PC: it takes the connection and
//! the cast as a panel would (as sbcast understands the protocol), receives the video and the
//! sound, and prints what arrives. It proves sbcast's side only, not a real panel's answers.
//!
//!   cargo run --example fake_panel -- [--port 7385] [--approve-after 2] [--drop-after 30]
//!   sbcast cast --host 127.0.0.1:7385
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

#[allow(dead_code)]
#[path = "../src/screenshare/wire.rs"]
mod wire;

use std::io::{self, Read};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use serde_json::{json, Value};
use wire::{read_message, write_message, ScreenshareMessage};

/// Receiver version sent back: from 63 on, video frames carry an index.
const VERSION: i32 = 63;

#[derive(Parser)]
struct Args {
    /// Command port (the first of the code table).
    #[arg(long, default_value_t = 7385)]
    port: u16,
    /// Seconds before "someone" allows the PC on the panel.
    #[arg(long, default_value_t = 2)]
    approve_after: u64,
    /// Cut every connection this many seconds into a cast, to try the reconnection.
    #[arg(long)]
    drop_after: Option<u64>,
}

/// Sockets of the current session, cut together by --drop-after.
type Sockets = Arc<Mutex<Vec<TcpStream>>>;

fn main() -> io::Result<()> {
    let args = Args::parse();
    let listener = TcpListener::bind(("0.0.0.0", args.port))?;
    println!("fake panel on port {}: sbcast cast --host 127.0.0.1:{}", args.port, args.port);
    for socket in listener.incoming() {
        let socket = socket?;
        println!("{} connected", socket.peer_addr()?);
        // One session at a time, as a panel showing one PC.
        if let Err(e) = session(socket, &args) {
            println!("session ended: {e}");
        }
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn send(socket: &Mutex<TcpStream>, (command_type, command_id): (i32, i32), sequence: i32, payload: &Value) -> io::Result<()> {
    let message = ScreenshareMessage { version: VERSION, command_type, command_id, sequence, timestamp: now_ms(), payload: payload.to_string() };
    write_message(&mut *socket.lock().unwrap(), &message)
}

fn session(socket: TcpStream, args: &Args) -> io::Result<()> {
    let sockets: Sockets = Arc::default();
    sockets.lock().unwrap().push(socket.try_clone()?);
    let writer = Arc::new(Mutex::new(socket.try_clone()?));
    let mut reader = socket;
    // A panel's heartbeat: sbcast drops a link that stays silent for 30 s.
    let beat = Arc::clone(&writer);
    std::thread::spawn(move || {
        while send(&beat, (0, 0), 0, &json!({"sentTimeMs": now_ms()})).is_ok() {
            std::thread::sleep(Duration::from_secs(3));
        }
    });
    loop {
        let message = read_message(&mut reader)?;
        let payload: Value = serde_json::from_str(&message.payload).unwrap_or(Value::Null);
        match (message.command_type, message.command_id) {
            (0, 0) => {}
            (1, 1) => {
                println!("connect request from {:?}, code {:?}: allowing in {} s", payload["deviceName"], payload["pinCode"], args.approve_after);
                std::thread::sleep(Duration::from_secs(args.approve_after));
                send(&writer, (1, 101), message.sequence, &json!({"resultType": 1, "receiverName": "Fake panel"}))?;
            }
            (6, 2) => {
                println!("cast request: video {}, audio {}", payload["video"], payload["supportAudio"]);
                let video = TcpListener::bind("0.0.0.0:0")?;
                let audio = TcpListener::bind("0.0.0.0:0")?;
                let answer = json!({"resultType": 1, "videoPort": video.local_addr()?.port(), "audioPort": audio.local_addr()?.port(), "bitrate": 0});
                send(&writer, (6, 102), message.sequence, &answer)?;
                receive(video, true, &sockets);
                receive(audio, false, &sockets);
                if let Some(after) = args.drop_after {
                    let sockets = Arc::clone(&sockets);
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(after));
                        println!("cutting every connection (--drop-after {after})");
                        for socket in sockets.lock().unwrap().iter() {
                            let _ = socket.shutdown(Shutdown::Both);
                        }
                    });
                }
            }
            (6, 3) => {
                println!("the PC stopped the cast");
                return Ok(());
            }
            ids => println!("message {ids:?}: {}", message.payload),
        }
    }
}

/// Accept one media connection on `listener` and count its frames in a thread.
fn receive(listener: TcpListener, video: bool, sockets: &Sockets) {
    let sockets = Arc::clone(sockets);
    std::thread::spawn(move || {
        let Ok((mut socket, _)) = listener.accept() else { return };
        if let Ok(clone) = socket.try_clone() {
            sockets.lock().unwrap().push(clone);
        }
        let what = if video { "video" } else { "audio" };
        let (frames, bytes, keys) = (AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0));
        let mut since = Instant::now();
        let ended = loop {
            match frame(&mut socket, video) {
                Ok(data) => {
                    frames.fetch_add(1, Ordering::Relaxed);
                    bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
                    // An IDR slice (NAL type 5) starts a key frame.
                    if video && data.windows(5).any(|w| w[..4] == [0, 0, 0, 1] && w[4] & 0x1f == 5) {
                        keys.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(e) => break e,
            }
            if since.elapsed() >= Duration::from_secs(5) {
                let seconds = since.elapsed().as_secs_f64();
                let (n, b, k) = (frames.swap(0, Ordering::Relaxed), bytes.swap(0, Ordering::Relaxed), keys.swap(0, Ordering::Relaxed));
                println!("{what}: {:.0} frames/s, {:.0} kbit/s, {k} key frames", n as f64 / seconds, b as f64 * 8.0 / 1000.0 / seconds);
                since = Instant::now();
            }
        };
        println!("{what} connection ended: {ended}");
    });
}

/// One frame: int32 length, int32 index (video), int32 type, then the data. Big endian.
fn frame(socket: &mut TcpStream, video: bool) -> io::Result<Vec<u8>> {
    let mut word = [0; 4];
    socket.read_exact(&mut word)?;
    let length = u32::from_be_bytes(word) as usize;
    if video {
        socket.read_exact(&mut word)?;
    }
    socket.read_exact(&mut word)?;
    let mut data = vec![0; length];
    socket.read_exact(&mut data)?;
    Ok(data)
}
