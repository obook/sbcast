//! tray/
//! Icon in the desktop's notification area (StatusNotifierItem, through ksni): the panels
//! found, the presets and the running cast's controls. Its items send actions to
//! session/tray_loop.rs, which runs the casts; this file only shows what the loop gives it.
//! The code a panel shows is typed in a kdialog box, and errors show as KDE notifications.
//!
//! Author: O. Booklage
//! Date: November 2026
//! Licence: MIT

use std::process::Command;
use std::sync::mpsc::Sender;

use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, ToolTip};

use crate::device::CastDevice;

/// What the user clicked.
pub enum Action {
    /// A listed panel: its code is asked first, unless it is an RTSP panel.
    Choose(CastDevice),
    /// Cast to a panel by its code alone.
    Code,
    /// The code typed for this panel (None: the panel the code gives).
    Coded(Option<CastDevice>, String),
    Stop,
    Freeze,
    PickScreen,
    Preset(usize),
    TerminalMenu,
    Quit,
}

/// What the tray shows, built by the loop from the menu state.
#[derive(Clone, Default, PartialEq)]
pub struct View {
    /// Panels found, each with whether it answered since the last scan.
    pub panels: Vec<(CastDevice, bool)>,
    /// Id of the panel being cast to.
    pub casting: Option<String>,
    pub frozen: bool,
    /// Index in menu::PRESETS; None for settings given by hand on the command line.
    pub preset: Option<usize>,
    pub status: String,
}

pub struct Tray {
    pub view: View,
    actions: Sender<Action>,
}

impl Tray {
    pub fn new(actions: Sender<Action>) -> Self {
        Self { view: View::default(), actions }
    }

    fn item(&self, label: &str, action: fn() -> Action) -> MenuItem<Self> {
        StandardItem { label: label.into(), activate: Box::new(move |t: &mut Self| t.send(action())), ..Default::default() }.into()
    }

    fn send(&self, action: Action) {
        // The loop is gone only when sbcast quits.
        let _ = self.actions.send(action);
    }
}

impl ksni::Tray for Tray {
    // A left click opens the menu too: the tray has no window to show.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "sbcast".into()
    }

    fn title(&self) -> String {
        "sbcast".into()
    }

    // Pixels, not the icon name: a name the theme lacks (sbcast not installed) shows nothing.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![ksni::Icon { width: 64, height: 64, data: include_bytes!("../../media/icon-64.argb").to_vec() }]
    }

    fn overlay_icon_name(&self) -> String {
        match (&self.view.casting, self.view.frozen) {
            (None, _) => "",
            (Some(_), false) => "media-record",
            (Some(_), true) => "media-playback-pause",
        }
        .into()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: "sbcast".into(), description: self.view.status.clone(), ..Default::default() }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = self
            .view
            .panels
            .iter()
            .map(|(panel, answered)| {
                let name = label(&panel.name);
                let panel = panel.clone();
                CheckmarkItem {
                    // The menu shows these in dark gray: there before, silent now.
                    label: if *answered { name } else { format!("{name} (not answering)") },
                    checked: self.view.casting.as_deref() == Some(panel.id.as_str()),
                    activate: Box::new(move |t: &mut Self| t.send(Action::Choose(panel.clone()))),
                    ..Default::default()
                }
                .into()
            })
            .collect();
        if items.is_empty() {
            items.push(StandardItem { label: "Searching for panels...".into(), enabled: false, ..Default::default() }.into());
        }
        items.push(self.item("Cast by code...", || Action::Code));
        items.push(MenuItem::Separator);
        if self.view.casting.is_some() {
            items.push(self.item("Stop", || Action::Stop));
            items.push(
                CheckmarkItem {
                    label: "Freeze".into(),
                    checked: self.view.frozen,
                    activate: Box::new(|t: &mut Self| t.send(Action::Freeze)),
                    ..Default::default()
                }
                .into(),
            );
            items.push(self.item("Choose screen...", || Action::PickScreen));
            items.push(MenuItem::Separator);
        }
        let options = crate::menu::PRESETS.iter().map(|(name, ..)| RadioItem { label: label(name), ..Default::default() }).collect();
        let presets = RadioGroup {
            // Out of range: no preset ticked.
            selected: self.view.preset.unwrap_or(usize::MAX),
            select: Box::new(|t: &mut Self, i| t.send(Action::Preset(i))),
            options,
        };
        items.push(SubMenu { label: "Preset".into(), submenu: vec![presets.into()], ..Default::default() }.into());
        items.push(self.item("Terminal menu", || Action::TerminalMenu));
        items.push(self.item("Quit", || Action::Quit));
        items
    }
}

/// A menu label shows `text` as is: an underscore alone marks the access key.
fn label(text: &str) -> String {
    text.replace('_', "__")
}

/// Ask for the code `panel` shows (None: a panel not listed) in a dialog, `last` filled in.
/// None when cancelled, or when kdialog is missing.
pub fn ask_code(panel: Option<&CastDevice>, last: &str) -> Option<String> {
    let prompt = match panel {
        Some(panel) => format!("Code shown on {} (empty if none):", panel.name),
        None => "Code shown on the panel:".to_string(),
    };
    let output = Command::new("kdialog").args(["--title", "sbcast", "--inputbox", &prompt, last]).output();
    match output {
        Ok(out) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("kdialog: {e}");
            None
        }
    }
}

/// Show `text` as a desktop notification for a few seconds, without waiting for it.
pub fn notify(text: &str) {
    let text = text.to_string();
    std::thread::spawn(move || {
        if let Err(e) = Command::new("kdialog").args(["--title", "sbcast", "--icon", "sbcast", "--passivepopup", &text, "8"]).status() {
            tracing::warn!("kdialog: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray as _;

    fn labels(tray: &Tray) -> Vec<String> {
        tray.menu()
            .into_iter()
            .filter_map(|item| match item {
                MenuItem::Standard(i) => Some(i.label),
                MenuItem::Checkmark(i) => Some(format!("{}{}", i.label, if i.checked { " [x]" } else { "" })),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn shows_panels_and_the_running_cast() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut tray = Tray::new(tx);
        assert_eq!(labels(&tray), ["Searching for panels...", "Cast by code...", "Terminal menu", "Quit"]);
        let panel = |id: &str, name: &str| CastDevice { id: id.into(), name: name.into(), model: "Panel".into(), ip: [10, 0, 0, 7].into(), port: 7385 };
        tray.view.panels = vec![(panel("a", "COL_A22"), true), (panel("b", "B22"), false)];
        tray.view.casting = Some("a".into());
        assert_eq!(
            labels(&tray),
            ["COL__A22 [x]", "B22 (not answering)", "Cast by code...", "Stop", "Freeze", "Choose screen...", "Terminal menu", "Quit"]
        );
    }
}
