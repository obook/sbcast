//! session/
//! How one cast runs and where its progress goes (the terminal or the menu). menu_loop.rs
//! runs the casts started from the menu; terminal.rs holds the terminal helpers (spinner,
//! panel choice, Ctrl+C).
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

mod menu_loop;
mod terminal;

use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;

use crate::capture::ScreenCast;
use crate::device::CastDevice;
use crate::menu;
pub use menu_loop::run_menu;
pub use terminal::{pick_device, spin, stop_flag, SEARCHING};

/// Where cast progress goes: the terminal, with spinners, or the menu.
#[derive(Clone)]
pub enum Out {
    Term,
    Menu(Sender<menu::Status>),
}

impl Out {
    pub fn say(&self, msg: &str) {
        match self {
            Out::Term => eprintln!("{msg}"),
            Out::Menu(_) => self.send(menu::Status::Info(msg.to_string())),
        }
    }

    /// Menu only: the terminal shows messages, not stats.
    pub fn send(&self, status: menu::Status) {
        if let Out::Menu(tx) = self {
            let _ = tx.send(status);
        }
    }
}

/// Cast `screen` to `device` until `stop` or the end of the cast.
pub fn cast_screen(device: &CastDevice, s: &menu::Settings, screen: &ScreenCast, stop: &'static AtomicBool, out: &Out) -> anyhow::Result<()> {
    tracing::info!(name = %device.name, ip = %device.ip, "casting");
    crate::screenshare::cast(SocketAddr::from((device.ip, device.port)), &device.name, s, screen, stop, out)
}
