//! menu_loop.rs
//! Run the menu: each Enter starts a cast in a thread while the menu stays on screen;
//! s stops it, q quits, p picks the screen again and moves the running cast to it.
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{cast_screen, stop_flag, Out};
use crate::capture::ScreenCast;
use crate::menu;

/// Run the menu until the user quits.
pub fn run_menu(mut settings: menu::Settings) -> anyhow::Result<()> {
    // Ctrl+C is a key in the menu; the flag still catches SIGTERM.
    let quit = stop_flag()?;
    // A fresh log for each run, open before the portal so its answer is logged too.
    if let Ok(file) = std::fs::File::create(crate::log_path()) {
        let _ = crate::MENU_LOG.set(file);
    }
    // The screen first, before the menu takes the terminal.
    let screen = ScreenCast::open()?;
    let mut terminal = ratatui::init();
    let stderr_to_log = crate::MENU_LOG.get().and_then(|file| super::terminal::StderrToFile::new(file).ok());
    // A panic's message went to the log with stderr: caught here, to say where it is.
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drive_menu(&mut terminal, &mut settings, screen, quit)));
    // Stderr back first: a final error must reach the user.
    drop(stderr_to_log);
    // Not restore(): its eprintln! panics when the terminal was closed.
    let _ = ratatui::try_restore();
    run.unwrap_or_else(|panic| {
        let _ = writeln!(std::io::stderr(), "sbcast stopped on an internal error: see {}", crate::log_path().display());
        std::panic::resume_unwind(panic)
    })
}

fn drive_menu(
    terminal: &mut ratatui::DefaultTerminal,
    s: &mut menu::Settings,
    screen: ScreenCast,
    quit: &'static AtomicBool,
) -> anyhow::Result<()> {
    let mut menu = menu::Menu::new();
    if let Some(note) = screen.note() {
        menu.set_status(menu::Status::Info(note));
    }
    // Chosen before the menu, kept for every cast; p replaces it.
    let screen = Arc::new(Mutex::new(Arc::new(screen)));
    // Stop flag of the running cast, and its thread.
    let mut running: Option<(&'static AtomicBool, JoinHandle<()>)> = None;
    // Error of a terminal closed without quitting: returned once the cast is stopped.
    let mut lost = None;
    loop {
        let action = match menu.next(terminal, s, quit) {
            Ok(action) => action,
            Err(e) => {
                tracing::warn!("terminal lost: {e:#}");
                lost = Some(e);
                menu::Action::Quit
            }
        };
        tracing::info!(?action, "menu action");
        if let menu::Action::PickScreen = action {
            pick_screen(&menu, &screen);
            continue;
        }
        if let Some((stop, cast)) = running.take() {
            menu.set_status(menu::Status::Info("Stopping...".into()));
            // The terminal may be closed: stopping the cast comes first.
            let _ = menu.draw_now(terminal, s);
            stop.store(true, Ordering::SeqCst);
            let started = Instant::now();
            wait_for(&cast);
            tracing::info!(finished = cast.is_finished(), waited_ms = started.elapsed().as_millis() as u64, "cast thread stopped");
            menu.set_status(menu::Status::Ended);
        }
        let device = match action {
            menu::Action::Quit => return lost.map_or(Ok(()), Err),
            menu::Action::Stop | menu::Action::PickScreen => continue,
            menu::Action::Cast(device) => device,
        };
        // ponytail: one leaked flag per cast (a byte), the cast wants a 'static flag.
        let stop: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        stop_on_signal(stop, quit);
        let out = Out::Menu(menu.status_sender());
        let settings = s.clone();
        let screen = Arc::clone(&screen.lock().unwrap());
        menu.set_casting(Some(device.id.clone()));
        let cast = std::thread::spawn(move || {
            let result = cast_screen(&device, &settings, &screen, stop, &out);
            tracing::info!(ok = result.is_ok(), "cast thread: cast ended");
            match result {
                Ok(()) => out.send(menu::Status::Ended),
                // The sharing may have been stopped from the desktop.
                Err(e) if format!("{e:#}").contains("capture") => {
                    out.send(menu::Status::Error(format!("{e:#} (p: choose the screen again)")))
                }
                Err(e) => out.send(menu::Status::Error(format!("{e:#}"))),
            }
            // Dropping the last copy of an old screen closes its portal session here.
            drop(screen);
            tracing::info!("cast thread: done");
        });
        running = Some((stop, cast));
    }
}

/// Stop the cast when `quit` is set by a signal, without the menu.
/// Closing the Konsole tab stops reading the terminal first: the menu then blocks while it
/// draws and never sees the signal.
fn stop_on_signal(stop: &'static AtomicBool, quit: &'static AtomicBool) {
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            if quit.load(Ordering::SeqCst) {
                stop.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
}

/// Open the portal dialog in a thread: the menu keeps answering, the cast goes on meanwhile.
/// Once a screen is chosen, the running cast restarts on it.
fn pick_screen(menu: &menu::Menu, screen: &Arc<Mutex<Arc<ScreenCast>>>) {
    let out = Out::Menu(menu.current_sender());
    let slot = Arc::clone(screen);
    std::thread::spawn(move || {
        out.say("Choose the screen in the dialog...");
        match ScreenCast::open() {
            Ok(opened) => {
                // A refused pointer is in the note.
                let said = opened.note().unwrap_or_else(|| "Screen and pointer granted.".to_string());
                *slot.lock().unwrap() = Arc::new(opened);
                out.say(&said);
                out.send(menu::Status::ScreenPicked);
            }
            // Not an Error status: that would end the stats of the running cast.
            Err(e) => out.say(&format!("Screen not changed: {e:#}")),
        }
    });
}

/// Wait for `thread`, at most 15 s: the panel can stop answering.
fn wait_for<T>(thread: &JoinHandle<T>) {
    let started = Instant::now();
    while !thread.is_finished() && started.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(50));
    }
}
