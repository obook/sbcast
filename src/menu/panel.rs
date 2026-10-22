//! panel.rs
//! Live panel of a running cast: duration, reconnections, lost frames, CPU and network on
//! the left, the bitrate chart on the right. The measures come from live.rs.
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

use std::sync::atomic::Ordering;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Sparkline};
use ratatui::Frame;

use super::draw::{rounded, ACCENT};
use super::live::{load_average, DROPPED_PER_MINUTE};
use super::Menu;

/// Lines of the Live panel, borders included.
pub(super) const LIVE_HEIGHT: u16 = 6;

impl Menu {
    pub(super) fn draw_live(&self, f: &mut Frame, area: Rect) {
        let live = &self.live;
        let [text_area, chart_area] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
        let label = |s: &str| Span::styled(format!("{s:<11}"), Style::new().fg(Color::Gray));
        let bold = |s: String, color: Color| Span::styled(s, Style::new().fg(color).add_modifier(Modifier::BOLD));

        let elapsed = live.started.map_or(0, |t| t.elapsed().as_secs());
        let dropped = live.dropped_last_minute();
        let network = live.network_dropped.as_ref().map(|n| n.load(Ordering::Relaxed));
        let warn = |bad: bool| if bad { Color::Yellow } else { Color::Green };
        let lines = vec![
            Line::from(vec![
                label("Duration"),
                bold(format!("{:02}:{:02}:{:02}", elapsed / 3600, elapsed / 60 % 60, elapsed % 60), Color::White),
            ]),
            Line::from(vec![
                label("Reconnects"),
                bold(live.reconnects.to_string(), warn(live.reconnects > 0)),
                Span::styled("   Lost frames ", Style::new().fg(Color::Gray)),
                bold(format!("{dropped} last minute"), warn(dropped > DROPPED_PER_MINUTE)),
            ]),
            Line::from(vec![
                label("CPU"),
                bold(format!("{:.0} % of a core", live.cpu), warn(live.cpu > 150.0)),
                Span::styled(format!("   load {}", load_average().unwrap_or_default()), Style::new().fg(Color::Gray)),
            ]),
            Line::from(vec![
                label("Network"),
                bold(network.map_or("-".into(), |n| format!("{n} frames dropped")), warn(network.is_some_and(|n| n > 0))),
            ]),
        ];
        f.render_widget(Paragraph::new(lines).block(rounded(false).title(" Live ")), text_area);

        let title = match live.bitrate.last() {
            Some(rate) => format!(" Bitrate {rate} kbit/s "),
            None => " Bitrate ".to_string(),
        };
        let chart = Sparkline::default()
            .block(rounded(false).title(title))
            .data(&live.bitrate)
            .style(Style::new().fg(ACCENT));
        f.render_widget(chart, chart_area);
    }
}
