//! tests.rs
//! Tests of the menu: settings that cycle, alert thresholds, and the screen drawn with
//! made-up panels.
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier};

use super::live::Live;
use super::settings::{change, cycle, value};
use super::*;
use crate::cli::Stream;

#[test]
fn cycles_and_wraps() {
    assert_eq!(cycle(&[1, 2, 3], 3, 1), 1);
    assert_eq!(cycle(&[1, 2, 3], 1, -1), 3);
    assert_eq!(cycle(&[1, 2, 3], 7, 1), 1);
}

#[test]
fn audio_delay_moves_by_50_ms_and_stops_at_the_ends() {
    let mut s = settings(2500);
    change(&mut s, AUDIO_DELAY_ROW, -1);
    assert_eq!(s.audio_delay, -50);
    s.audio_delay = -500;
    change(&mut s, AUDIO_DELAY_ROW, -1);
    assert_eq!(s.audio_delay, -500);
    s.audio_delay = 1500;
    change(&mut s, AUDIO_DELAY_ROW, 1);
    assert_eq!(s.audio_delay, 1500);
}

#[test]
fn alerts_follow_thresholds() {
    let mut live = Live::default();
    assert!(live.alert().is_none());
    live.dropped_history = VecDeque::from([0, 45]);
    assert!(live.alert().unwrap().0.starts_with("PC too slow"));
}

fn device(id: &str, name: &str) -> CastDevice {
    CastDevice { id: id.into(), name: name.into(), model: "Panel".into(), ip: std::net::Ipv4Addr::LOCALHOST, port: 7385 }
}

fn settings(bitrate: u32) -> Settings {
    let stream = Stream { resolution: (1920, 1080), fps: 30, bitrate, no_audio: false };
    Settings { stream, audio_delay: 0, pin: None, name: "PC guest".into() }
}

#[test]
fn long_device_list_scrolls() {
    let mut menu = Menu::idle();
    for i in 0..30 {
        let id = format!("d{i:02}");
        menu.devices.insert(id.clone(), device(&id, &format!("Room {i:02}")));
    }
    menu.device.select(Some(25));
    let s = settings(2500);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
    terminal.draw(|f| menu.draw(f, &s)).unwrap();
    let screen: String = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    assert!(screen.contains("Panels (30)"));
    // The selected panel is scrolled into view, and the scrollbar thumb is drawn.
    assert!(screen.contains("Room 25") && screen.contains('█'));
}

#[test]
fn draws_and_applies_presets() {
    let mut s = settings(4000);
    change(&mut s, 0, -1);
    assert_eq!(value(&s, 0), "Default");
    assert_eq!(s.stream.bitrate, 2500);

    // Made-up panels: no network search, so no real panel shows up.
    let mut menu = Menu::idle();
    for (id, name) in [("p1", "Room 104"), ("p2", "Room 105"), ("p3", "Library")] {
        menu.devices.insert(id.into(), device(id, name));
    }
    menu.device.select(Some(1));
    menu.set_casting(Some("p1".into()));
    menu.set_status(Status::Info("Casting to Room 104".into()));
    menu.live.started = Instant::now().checked_sub(Duration::from_secs(754));
    menu.live.cpu = 62.0;
    menu.live.bitrate = (0..60).map(|i| 2500 + (i * 37 % 11) * 40 - 200).collect();

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| menu.draw(f, &s)).unwrap();
    let buffer = terminal.backend().buffer();
    let screen: Vec<&str> = buffer.content().iter().map(|c| c.symbol()).collect();
    for line in screen.chunks(80) {
        println!("{}", line.concat());
    }
    let text = screen.concat();
    assert!(text.contains("Room 104") && text.contains("Default") && text.contains("Audio delay"));
    // The key help fits in 80 columns during a cast.
    assert!(text.contains("s stop") && text.contains("q quit"));
    assert!(text.contains("00:12:34") && text.contains("Bitrate"));

    // SBCAST_SCREENSHOT=file.html cargo test draws: the same screen as a colored page, for media/.
    if let Ok(path) = std::env::var("SBCAST_SCREENSHOT") {
        std::fs::write(path, html(buffer)).unwrap();
    }
}

fn html(buffer: &ratatui::buffer::Buffer) -> String {
    let css = |c: Color, default: &str| match c {
        Color::Reset => default.to_string(),
        Color::Black => "#1e1e2e".into(),
        Color::Gray => "#a6adc8".into(),
        Color::DarkGray => "#585b70".into(),
        Color::Green => "#a6e3a1".into(),
        Color::Yellow => "#f9e2af".into(),
        Color::Red => "#f38ba8".into(),
        Color::Cyan => "#89dceb".into(),
        Color::White => "#ffffff".into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => default.to_string(),
    };
    let mut out = String::from(
        "<!doctype html><meta charset=utf-8><body style=\"margin:0;background:#1e1e2e\">\
         <pre style=\"margin:16px;font:16px/1.15 'DejaVu Sans Mono',monospace;color:#cdd6f4\">",
    );
    for (i, cell) in buffer.content().iter().enumerate() {
        if i > 0 && i % buffer.area.width as usize == 0 {
            out.push('\n');
        }
        let weight = if cell.modifier.contains(Modifier::BOLD) { "bold" } else { "normal" };
        let symbol = match cell.symbol() {
            "<" => "&lt;",
            ">" => "&gt;",
            "&" => "&amp;",
            s => s,
        };
        out.push_str(&format!(
            "<span style=\"color:{};background:{};font-weight:{weight}\">{symbol}</span>",
            css(cell.fg, "#cdd6f4"),
            css(cell.bg, "transparent"),
        ));
    }
    out + "</pre>"
}
