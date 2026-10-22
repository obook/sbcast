//! live.rs
//! Measures of the running cast, refreshed once a second: bitrate, lost frames, CPU.
//! Turns them into the alert shown under the status line. panel.rs draws them.
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::style::Color;

use super::ByteCounter;

// ponytail: first guess, to adjust with tests on a real panel.
/// One second of video lost per minute.
pub(super) const DROPPED_PER_MINUTE: u64 = 30;

/// Stats of the current cast, shown in the Live panel.
#[derive(Default)]
pub(super) struct Live {
    pub(super) started: Option<Instant>,
    pub(super) reconnects: u32,
    /// Byte counter of the sent stream, and its last reading.
    pub(super) served: Option<(ByteCounter, Option<(u64, Instant)>)>,
    /// kbit/s, one sample per second, last minute.
    pub(super) bitrate: Vec<u64>,
    /// sbcast's CPU time in clock ticks, and when it was read.
    cpu_ticks: Option<(u64, Instant)>,
    /// Percent of one core.
    pub(super) cpu: f64,
    sampled: Option<Instant>,
    pub(super) dropped: Option<Arc<AtomicU64>>,
    /// Frames the network did not take in time, since the start.
    pub(super) network_dropped: Option<Arc<AtomicU64>>,
    /// Dropped frames total, one sample per second, last minute.
    pub(super) dropped_history: VecDeque<u64>,
    /// Seconds sampled, to log a summary every 10.
    samples: u64,
}

impl Live {
    /// Fresh stats for a cast starting now.
    pub(super) fn starting() -> Self {
        Self { started: Some(Instant::now()), ..Self::default() }
    }

    /// Refresh the measures once a second.
    pub(super) fn sample(&mut self) {
        if self.sampled.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.sampled = Some(Instant::now());
        if let Some((counter, last)) = &mut self.served {
            let bytes = counter();
            if let Some((before, at)) = *last {
                self.bitrate.push((bytes.saturating_sub(before) as f64 * 8.0 / 1000.0 / at.elapsed().as_secs_f64()) as u64);
            }
            *last = Some((bytes, Instant::now()));
        }
        if self.bitrate.len() > 60 {
            self.bitrate.remove(0);
        }
        if let Some(dropped) = &self.dropped {
            self.dropped_history.push_back(dropped.load(Ordering::Relaxed));
            if self.dropped_history.len() > 61 {
                self.dropped_history.pop_front();
            }
        }
        self.samples += 1;
        if self.samples.is_multiple_of(10) {
            tracing::info!(lost_last_minute = self.dropped_last_minute(), cpu = self.cpu as u64, "live");
        }
        if let Some(ticks) = cpu_ticks() {
            if let Some((before, at)) = self.cpu_ticks {
                // ponytail: assumes 100 ticks per second (the Linux default), read sysconf if it ever differs.
                self.cpu = (ticks - before) as f64 / at.elapsed().as_secs_f64();
            }
            self.cpu_ticks = Some((ticks, Instant::now()));
        }
    }

    pub(super) fn dropped_last_minute(&self) -> u64 {
        match (self.dropped_history.front(), self.dropped_history.back()) {
            (Some(first), Some(last)) => last - first,
            _ => 0,
        }
    }

    /// The worst problem seen, with its color, or None when all is well.
    pub(super) fn alert(&self) -> Option<(String, Color)> {
        let dropped = self.dropped_last_minute();
        if dropped > DROPPED_PER_MINUTE {
            return Some((format!("PC too slow: {dropped} frames lost in a minute. Lower the resolution or close programs"), Color::Red));
        }
        None
    }
}

/// utime + stime of this process, from /proc/self/stat (fields 14 and 15).
fn cpu_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The command name can hold spaces: count fields after its closing parenthesis.
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace().skip(11);
    Some(fields.next()?.parse::<u64>().ok()? + fields.next()?.parse::<u64>().ok()?)
}

/// System load over the last minute, out of the number of CPUs (e.g. "1.20/8").
pub(super) fn load_average() -> Option<String> {
    let load = std::fs::read_to_string("/proc/loadavg").ok()?;
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    Some(format!("{}/{cpus}", load.split_whitespace().next()?))
}
