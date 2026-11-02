//! discovery.rs
//! Find the interactive panels (service `_sspcast._tcp`) on the local network with mDNS,
//! and the EShare panels (`_eshareserver._tcp`, and the UDP broadcast of the EShare client),
//! listed but not castable yet.
//! Used by `sbcast list`, by `--device` and by the menu's list.
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};

/// Unverified on a real panel.
const SERVICE_TYPE: &str = "_sspcast._tcp.local.";
/// EShare panels, seen at school (2026-11-02): TXT `sn=eshare_retail_pro`, `pin=<code>`.
const ESHARE_TYPE: &str = "_eshareserver._tcp.local.";
/// Model shown for an EShare panel; casting refuses it until its protocol is written.
pub const ESHARE: &str = "EShare";
/// EShare command port, from the panels' mDNS answers.
const ESHARE_PORT: u16 = 57395;
/// UDP port of the EShare client's search: panels answer "<model>:<name>:8121:25123:56789:sn=...".
/// Most panels of the school answer it, where few answer mDNS on the Wi-Fi (2026-11-02).
const ESHARE_SEARCH_PORT: u16 = 48689;
const ESHARE_SEARCH_WORD: &[u8] = b"FindECloudBox";

/// A panel, reached on its command port `port`.
#[derive(Debug, Clone)]
pub struct CastDevice {
    pub id: String,
    pub name: String,
    pub model: String,
    pub ip: Ipv4Addr,
    pub port: u16,
}

impl CastDevice {
    fn from_service(info: &ResolvedService) -> Option<Self> {
        let ip = info.get_addresses_v4().into_iter().next();
        if info.ty_domain == ESHARE_TYPE {
            return Self::eshare(info.get_fullname(), ip?, info.get_port());
        }
        let txt = |key: &str| info.get_property_val_str(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Self::from_txt(txt, ip, info.get_port())
    }

    /// EShare panel: its instance name (the room, e.g. COL-A11) is the shown name.
    fn eshare(fullname: &str, ip: Ipv4Addr, port: u16) -> Option<Self> {
        let name = fullname.strip_suffix(ESHARE_TYPE)?.trim_end_matches('.');
        Some(Self::eshare_named(name, ip, port))
    }

    /// Keyed by room, not address: a panel that gets a new address replaces its old line.
    fn eshare_named(name: &str, ip: Ipv4Addr, port: u16) -> Self {
        Self { id: format!("eshare:{name}"), name: name.into(), model: ESHARE.into(), ip, port }
    }

    /// EShare panel from its answer to the UDP search.
    fn from_eshare_reply(reply: &[u8], ip: Ipv4Addr) -> Option<Self> {
        let text = std::str::from_utf8(reply).ok()?;
        let name = text.split(':').nth(1).map(str::trim).filter(|n| !n.is_empty())?;
        Some(Self::eshare_named(name, ip, ESHARE_PORT))
    }

    /// Panel from its TXT record, its address (A record) and its SRV port.
    fn from_txt(txt: impl Fn(&str) -> Option<String>, ip: Option<Ipv4Addr>, port: u16) -> Option<Self> {
        // `host` and `port` from the TXT record win over the A and SRV records.
        // Other keys: mac, pv, dv (versions, hypothesis), useHost.
        let ip = txt("host").and_then(|h| h.parse().ok()).or(ip)?;
        let port = txt("port").and_then(|p| p.parse().ok()).unwrap_or(port);
        let name = txt("name").unwrap_or_else(|| ip.to_string());
        let id = txt("id").unwrap_or_else(|| format!("{ip}:{port}"));
        Some(Self { id, name, model: "Panel".into(), ip, port })
    }
}

impl CastDevice {
    /// One line of the panels file: id, name, model, ip, port, tab-separated.
    pub fn to_line(&self) -> String {
        format!("{}\t{}\t{}\t{}\t{}", self.id, self.name, self.model, self.ip, self.port)
    }

    fn from_line(line: &str) -> Option<Self> {
        let [id, name, model, ip, port] = line.split('\t').collect::<Vec<_>>().try_into().ok()?;
        Some(Self { id: id.into(), name: name.into(), model: model.into(), ip: ip.parse().ok()?, port: port.parse().ok()? })
    }
}

/// Panels seen before, kept because a busy network (a school at 10 am) may not let their
/// answers through: ~/.local/state/sbcast/panels.tsv.
fn panels_file() -> std::path::PathBuf {
    crate::screenshare::trace::path().with_file_name("panels.tsv")
}

/// Panels seen on earlier runs; none when the file is missing or unreadable.
pub fn remembered() -> Vec<CastDevice> {
    let text = std::fs::read_to_string(panels_file()).unwrap_or_default();
    text.lines().filter_map(CastDevice::from_line).collect()
}

/// Write the panels seen so far. A failure only costs the memory, so it is logged and skipped.
// ponytail: panels never leave the file; delete it by hand if a room is gone.
pub fn remember<'a>(devices: impl IntoIterator<Item = &'a CastDevice>) {
    let path = panels_file();
    let text: String = devices.into_iter().map(|d| d.to_line() + "\n").collect();
    let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, text));
    if let Err(e) = written {
        tracing::warn!(path = %path.display(), "cannot remember the panels: {e}");
    }
}

/// Browse the network for `timeout` and return the panels found, sorted by name.
pub fn discover(timeout: Duration) -> anyhow::Result<Vec<CastDevice>> {
    let mut found = BTreeMap::new();
    browse(timeout, |device| {
        // Keyed by id: a panel answers several times during the browse.
        found.insert(device.id.clone(), device);
        false
    })?;
    let mut devices: Vec<_> = found.into_values().collect();
    devices.sort_by_key(|d| d.name.to_lowercase());
    Ok(devices)
}

/// Find the panel whose name (case-insensitive) or id is `query`.
/// Returns as soon as it answers, instead of waiting for the full timeout.
/// When it does not answer, the error lists the panels seen during the same browse.
pub fn find(query: &str, timeout: Duration) -> anyhow::Result<CastDevice> {
    let mut hit = None;
    let mut seen = BTreeMap::new();
    browse(timeout, |device| {
        if is_named(&device, query) {
            hit = Some(device);
            return true;
        }
        seen.insert(device.id.clone(), device.name);
        false
    })?;
    if let Some(device) = hit {
        return Ok(device);
    }
    if seen.is_empty() {
        anyhow::bail!("{NONE_FOUND}");
    }
    let names: Vec<_> = seen.into_values().collect();
    anyhow::bail!("panel {query:?} not found; available: {}", names.join(", "))
}

/// True when `query` is the panel's id, or its name in any case (accented letters included).
fn is_named(device: &CastDevice, query: &str) -> bool {
    device.id == query || device.name.to_lowercase() == query.to_lowercase()
}

pub const NONE_FOUND: &str = "no panel found (check that mDNS/UDP 5353 is not blocked, or use --pin)";

/// Send the EShare client's search to the local network; panels answer on the same port.
/// None when the port cannot be opened: the mDNS search still runs.
// ponytail: one broadcast to 255.255.255.255 leaves by the default route only; send it on each
// interface (getifaddrs) if the PC has panels on two networks.
fn eshare_search() -> Option<UdpSocket> {
    let opened = (|| {
        let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)?;
        // Shared: an EShare client running on the PC holds the same port.
        socket.set_reuse_address(true)?;
        socket.set_broadcast(true)?;
        socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, ESHARE_SEARCH_PORT)).into())?;
        let socket: UdpSocket = socket.into();
        socket.set_nonblocking(true)?;
        socket.send_to(&eshare_search_message(), (Ipv4Addr::BROADCAST, ESHARE_SEARCH_PORT))?;
        std::io::Result::Ok(socket)
    })();
    opened.inspect_err(|e| tracing::warn!("EShare search not sent: {e}")).ok()
}

/// 50 bytes: the word's length (big-endian) at 12, the word at 20.
fn eshare_search_message() -> [u8; 50] {
    let mut message = [0u8; 50];
    message[12..16].copy_from_slice(&(ESHARE_SEARCH_WORD.len() as u32).to_be_bytes());
    message[20..20 + ESHARE_SEARCH_WORD.len()].copy_from_slice(ESHARE_SEARCH_WORD);
    message
}

/// Call `on_device` for each resolved panel until it returns true or `timeout` expires.
pub fn browse(timeout: Duration, on_device: impl FnMut(CastDevice) -> bool) -> anyhow::Result<()> {
    let mdns = ServiceDaemon::new()?;
    let result = browse_with(&mdns, timeout, on_device);
    let _ = mdns.shutdown();
    result.map(|_| ())
}

/// One round of `browse` on a daemon kept by the caller: its cache keeps the answers of earlier
/// rounds, which a busy network gives out a few at a time. Returns true when `on_device` stopped it.
pub fn browse_with(mdns: &ServiceDaemon, timeout: Duration, mut on_device: impl FnMut(CastDevice) -> bool) -> anyhow::Result<bool> {
    let browsers = [mdns.browse(SERVICE_TYPE)?, mdns.browse(ESHARE_TYPE)?];
    let eshare = eshare_search();
    let mut stopped = false;
    let deadline = Instant::now() + timeout;
    // ponytail: polls the two browsers every 50 ms; a select would wake at once, not worth a dependency.
    'browse: while Instant::now() < deadline {
        for browser in &browsers {
            while let Ok(event) = browser.try_recv() {
                if let ServiceEvent::ServiceResolved(info) = event {
                    tracing::debug!(?info, "resolved");
                    if CastDevice::from_service(&info).is_some_and(&mut on_device) {
                        stopped = true;
                        break 'browse;
                    }
                }
            }
        }
        let mut reply = [0u8; 512];
        while let Some(Ok((len, SocketAddr::V4(from)))) = eshare.as_ref().map(|s| s.recv_from(&mut reply)) {
            if CastDevice::from_eshare_reply(&reply[..len], *from.ip()).is_some_and(&mut on_device) {
                stopped = true;
                break 'browse;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The next round's browse asks the network again and replays the cache.
    for ty in [SERVICE_TYPE, ESHARE_TYPE] {
        let _ = mdns.stop_browse(ty);
    }
    Ok(stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_names_in_any_case() {
        let device =
            CastDevice { id: "abc123".into(), name: "TÉLÉ Salle 12".into(), model: "Panel".into(), ip: Ipv4Addr::LOCALHOST, port: 7385 };
        assert!(is_named(&device, "télé salle 12"));
        assert!(is_named(&device, "abc123"));
        assert!(!is_named(&device, "Salle 12"));
    }

    #[test]
    fn names_an_eshare_panel_after_its_room() {
        let ip = Ipv4Addr::new(10, 116, 7, 254);
        let device = CastDevice::eshare("COL-A11._eshareserver._tcp.local.", ip, 57395).unwrap();
        assert_eq!((device.name.as_str(), device.model.as_str(), device.id.as_str()), ("COL-A11", ESHARE, "eshare:COL-A11"));
        let line = device.to_line();
        assert_eq!(CastDevice::from_line(&line).unwrap().to_line(), line);
        assert!(CastDevice::from_line("COL-A11\tbroken").is_none());
        assert!(CastDevice::eshare("COL-A11._sspcast._tcp.local.", ip, 57395).is_none());
    }

    #[test]
    fn reads_an_eshare_search_reply() {
        let ip = Ipv4Addr::new(10, 116, 11, 20);
        let device = CastDevice::from_eshare_reply(b"ECloudBox516Pro:COL-A22:8121:25123:56789:sn=0x103001ff", ip).unwrap();
        assert_eq!((device.id.as_str(), device.name.as_str(), device.ip, device.port), ("eshare:COL-A22", "COL-A22", ip, 57395));
        assert!(CastDevice::from_eshare_reply(b"FindECloudBox", ip).is_none());
        let message = eshare_search_message();
        assert_eq!((message[15], &message[20..33]), (13, &b"FindECloudBox"[..]));
    }

    #[test]
    fn reads_a_panel_txt_record() {
        let record = [("id", "0a1b2c"), ("name", "Salle 104"), ("port", "15000"), ("pv", "76"), ("useHost", "false")];
        let txt = |key: &str| record.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string());
        let device = CastDevice::from_txt(txt, Some(Ipv4Addr::new(10, 0, 0, 7)), 9000).unwrap();
        assert_eq!((device.id.as_str(), device.name.as_str(), device.port), ("0a1b2c", "Salle 104", 15000));
        assert_eq!(device.ip, Ipv4Addr::new(10, 0, 0, 7));
        // Without TXT keys: the SRV port, and the address as name and id.
        let bare = CastDevice::from_txt(|_| None, Some(Ipv4Addr::new(10, 0, 0, 8)), 9000).unwrap();
        assert_eq!((bare.name.as_str(), bare.id.as_str(), bare.port), ("10.0.0.8", "10.0.0.8:9000", 9000));
    }
}
