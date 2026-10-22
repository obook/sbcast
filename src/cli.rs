//! cli.rs
//! Command line options (clap): the commands pin, list and cast, and the stream settings.
//! The menu starts from these values.
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

use clap::{Args, Parser, Subcommand};

/// Cast the Linux desktop to an interactive panel, with the panel's touches back on the PC.
#[derive(Parser)]
#[command(name = "sbcast", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Show the address a panel's code gives, without connecting.
    Pin {
        /// The 6-character code shown on the panel.
        code: String,
    },
    /// List the interactive panels on the local network.
    List {
        /// Discovery duration in seconds.
        #[arg(long, default_value_t = 8)]
        timeout: u64,
    },
    /// Cast the screen to a panel.
    Cast {
        /// Panel name or id. Without it, choose from a list.
        #[arg(long)]
        device: Option<String>,
        /// Delay the sound by this many milliseconds, if it comes before the image on the panel.
        /// Negative: delay the image instead, if the sound comes after it.
        /// Changed in the menu during a cast, it applies at the next cast.
        #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
        audio_delay: i64,
        /// The panel's address and command port, when --pin cannot give it.
        #[arg(long, conflicts_with = "device")]
        host: Option<std::net::SocketAddr>,
        /// The 6-character code shown on the panel. It also gives the panel's address
        /// (unverified on a real panel).
        #[arg(long)]
        pin: Option<String>,
        /// Name the panel shows when it asks to accept this PC.
        #[arg(long, default_value = "PC guest")]
        name: String,
        #[command(flatten)]
        stream: Stream,
    },
}

/// Capture settings.
#[derive(Args, Clone)]
pub struct Stream {
    /// Output size, letterboxed if the screen has another shape.
    #[arg(long, default_value = "1920x1080", value_parser = parse_resolution)]
    pub resolution: (u32, u32),
    /// Frames per second.
    #[arg(long, default_value_t = 30)]
    pub fps: u32,
    /// Video bitrate in kbit/s.
    #[arg(long, default_value_t = 2500)]
    pub bitrate: u32,
    /// No sound.
    #[arg(long)]
    pub no_audio: bool,
}

fn parse_resolution(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once('x').ok_or("expected WIDTHxHEIGHT, e.g. 1280x720")?;
    let size = |v: &str| v.parse::<u32>().ok().filter(|n| *n > 0 && n % 2 == 0);
    match (size(w), size(h)) {
        (Some(w), Some(h)) => Ok((w, h)),
        _ => Err("width and height must be even positive numbers".into()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_resolution() {
        assert_eq!(super::parse_resolution("1280x720"), Ok((1280, 720)));
        assert!(super::parse_resolution("1280").is_err());
        assert!(super::parse_resolution("1281x720").is_err());
        assert!(super::parse_resolution("0x720").is_err());
    }
}
