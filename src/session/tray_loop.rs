//! tray_loop.rs
//! Run the tray: take its clicks and the menu state (panels found, the running cast's
//! progress) and show them on the icon. One cast at a time, as in the menu. The screen is
//! shared only while a cast runs, so the desktop's sharing indicator goes off between casts;
//! the screen dialogs then come back at each cast.
//!
//! Author: O. Booklage
//! Date: November 2026
//! Licence: MIT

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context;
use ksni::blocking::TrayMethods;

use super::menu_loop::{pick_screen, start_cast, wait_for};
use super::{stop_flag, Out};
use crate::device::{discovery::RTSP_PANEL, CastDevice};
use crate::menu;
use crate::tray::{self, Action};

/// Run the tray until Quit, or the end of the session.
pub fn run_tray(mut settings: menu::Settings) -> anyhow::Result<()> {
    let quit = stop_flag()?;
    // Started again from the application menu, a second tray would show a second icon.
    let lock = std::fs::File::create(crate::runtime_file("sbcast-tray.lock"))?;
    if lock.try_lock().is_err() {
        eprintln!("sbcast tray is already running.");
        return Ok(());
    }
    if let Ok(file) = std::fs::File::create(crate::runtime_file("sbcast-tray.log")) {
        let _ = crate::MENU_LOG.set(file);
    }
    let (tx, actions) = mpsc::channel();
    // Started with the session, the tray may come before the desktop's panel: it waits for it.
    let icon = tray::Tray::new(tx.clone()).assume_sni_available(true).spawn().context("no notification area")?;
    let mut menu = menu::Menu::new();
    let screen = Arc::new(Mutex::new(None));
    let mut running: Option<(&'static AtomicBool, JoinHandle<()>)> = None;
    let mut shown = None;
    // Last error notified: each shows once.
    let mut told = String::new();
    'run: while !quit.load(Ordering::SeqCst) {
        let mut cast = match menu.poll(&settings) {
            // A new screen picked for the running cast restarts it there.
            Some(menu::Action::Cast(device)) => Some(device),
            _ => None,
        };
        while let Ok(action) = actions.try_recv() {
            match action {
                // An RTSP panel gives its code to sbcast: nothing to type.
                Action::Choose(device) if device.model == RTSP_PANEL => cast = Some(device),
                Action::Choose(device) => ask_code(Some(device), &settings, &tx),
                Action::Code => ask_code(None, &settings, &tx),
                Action::Coded(_, code) if !menu::code::valid(&code) => tray::notify("A panel code has letters and digits only."),
                Action::Coded(panel, code) => {
                    settings.pin = (!code.is_empty()).then(|| code.clone());
                    // A panel not in the list: its code gives its address.
                    match panel.map_or_else(|| menu::code::panel(&code), Ok) {
                        Ok(device) => cast = Some(device),
                        Err(e) => tray::notify(&e),
                    }
                }
                Action::Stop => stop(&mut running, &mut menu),
                Action::Freeze => drop(crate::capture::toggle_freeze()),
                Action::PickScreen => pick_screen(Out::Menu(menu.current_sender()), &screen),
                Action::Preset(i) => (_, settings.stream.resolution, settings.stream.bitrate) = menu::PRESETS[i],
                Action::TerminalMenu => terminal_menu(),
                Action::Quit => break 'run,
            }
        }
        if let Some(device) = cast {
            tracing::info!(name = %device.name, "tray: cast");
            stop(&mut running, &mut menu);
            told.clear();
            running = Some(start_cast(&mut menu, device, &settings, &screen, quit));
        }
        // A cast that ended by itself (refused, panel lost) frees the screen too.
        if running.as_ref().is_some_and(|(_, cast)| cast.is_finished()) {
            running = None;
        }
        if running.is_none() {
            // The last copy: dropping it closes the portal session.
            *screen.lock().unwrap() = None;
        }
        if let Some((error, true)) = menu.status_line().filter(|(text, _)| *text != told) {
            told = error.to_string();
            tray::notify(error);
        }
        let view = view(&menu, &settings);
        if shown.as_ref() != Some(&view) {
            if icon.update(|t| t.view = view.clone()).is_none() {
                break;
            }
            shown = Some(view);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    stop(&mut running, &mut menu);
    icon.shutdown().wait();
    Ok(())
}

/// What the icon shows of the menu state.
fn view(menu: &menu::Menu, s: &menu::Settings) -> tray::View {
    let status = match (menu.casting(), menu.status_line()) {
        (_, Some((text, true))) | (Some(_), Some((text, false))) => text.to_string(),
        // Not the menu's "Choose a panel, then Enter".
        _ => "Not casting".to_string(),
    };
    tray::View {
        panels: menu.panels(),
        casting: menu.casting().map(str::to_string),
        frozen: crate::capture::FREEZE.load(Ordering::SeqCst),
        preset: menu::preset(s),
        status,
    }
}

/// Stop the running cast, if any, and wait for it.
fn stop(running: &mut Option<(&'static AtomicBool, JoinHandle<()>)>, menu: &mut menu::Menu) {
    if let Some((stop, cast)) = running.take() {
        tracing::info!("tray: stop");
        stop.store(true, Ordering::SeqCst);
        wait_for(&cast);
        menu.set_status(menu::Status::Ended);
    }
}

/// Ask for the code in a dialog, in a thread: the icon keeps answering meanwhile.
fn ask_code(panel: Option<CastDevice>, s: &menu::Settings, tx: &Sender<Action>) {
    let (last, tx) = (s.pin.clone().unwrap_or_default(), tx.clone());
    std::thread::spawn(move || {
        if let Some(code) = tray::ask_code(panel.as_ref(), &last) {
            let _ = tx.send(Action::Coded(panel, code));
        }
    });
}

/// Open the terminal menu in Konsole.
// shortcut: Konsole only (KDE), switch to xdg-terminal-exec once the distributions ship it.
fn terminal_menu() {
    // After an upgrade, the running file is gone: the one installed then.
    let sbcast = std::env::current_exe().ok().filter(|exe| exe.exists()).unwrap_or_else(|| "sbcast".into());
    match Command::new("konsole").arg("-e").arg(sbcast).spawn() {
        Ok(mut konsole) => drop(std::thread::spawn(move || konsole.wait())),
        Err(e) => tray::notify(&format!("Konsole did not start: {e}")),
    }
}
