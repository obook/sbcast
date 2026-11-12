//! discovery.rs
//! Find the interactive panels (service `_sspcast._tcp`) on the local network with mDNS,
//! and the RTSP panels (their own mDNS service, and a UDP broadcast search).
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
/// RTSP panels, seen at school (2026-11-02). This service type and the search word below are
/// the values the panels answer to: they must stay as they are.
const RTSP_SERVICE_TYPE: &str = "_eshareserver._tcp.local.";
/// Model shown for an RTSP panel; session/ casts to it with rtsp/.
pub const RTSP_PANEL: &str = "RTSP panel";
/// Command port of an RTSP panel, from the panels' mDNS answers.
const RTSP_PANEL_PORT: u16 = 57395;
/// UDP port of the panels' search: they answer "<model>:<name>:8121:25123:56789:sn=...".
/// Most panels of the school answer it, where few answer mDNS on the Wi-Fi (2026-11-02).
const SEARCH_PORT: u16 = 48689;
const SEARCH_WORD: &[u8] = b"FindECloudBox";

/// A panel, reached on its command port `port`.
#[derive(Debug, Clone, PartialEq)]
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
        if info.ty_domain == RTSP_SERVICE_TYPE {
            return Self::rtsp_panel(info.get_fullname(), ip?, info.get_port());
        }
        let txt = |key: &str| info.get_property_val_str(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Self::from_txt(txt, ip, info.get_port())
    }

    /// RTSP panel: its instance name (the room, e.g. COL-A11) is the shown name.
    fn rtsp_panel(fullname: &str, ip: Ipv4Addr, port: u16) -> Option<Self> {
        let name = fullname.strip_suffix(RTSP_SERVICE_TYPE)?.trim_end_matches('.');
        Some(Self::rtsp_named(name, ip, port))
    }

    /// Keyed by room, not address: a panel that gets a new address replaces its old line.
    fn rtsp_named(name: &str, ip: Ipv4Addr, port: u16) -> Self {
        Self { id: format!("rtsp:{name}"), name: name.into(), model: RTSP_PANEL.into(), ip, port }
    }

    /// RTSP panel from its answer to the UDP search.
    fn from_search_reply(reply: &[u8], ip: Ipv4Addr) -> Option<Self> {
        let text = std::str::from_utf8(reply).ok()?;
        let name = text.split(':').nth(1).map(str::trim).filter(|n| !n.is_empty())?;
        Some(Self::rtsp_named(name, ip, RTSP_PANEL_PORT))
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
    /// One line of the panels file: id, name, model, ip, port, date of the last answer
    /// (Unix seconds), tab-separated.
    fn to_line(&self, seen: u64) -> String {
        format!("{}\t{}\t{}\t{}\t{}\t{seen}", self.id, self.name, self.model, self.ip, self.port)
    }

    fn from_line(line: &str, now: u64) -> Option<(Self, u64)> {
        let cols: Vec<_> = line.split('\t').collect();
        let (cols, seen) = match cols.as_slice() {
            [cols @ .., seen] if cols.len() == 5 => (cols, seen.parse().ok()?),
            // A line written before the dates: counts as seen now, to start its 3 weeks.
            cols => (cols, now),
        };
        let [id, name, model, ip, port]: [&str; 5] = cols.try_into().ok()?;
        let device = Self { id: id.into(), name: name.into(), model: model.into(), ip: ip.parse().ok()?, port: port.parse().ok()? };
        Some((device, seen))
    }
}

/// A panel silent this long is forgotten: removed, or on a network not visited lately.
const FORGET_AFTER: u64 = 21 * 24 * 3600;
/// A date moves at most once a day, so answers every 12 s do not rewrite the file.
const DATE_STEP: u64 = 24 * 3600;

/// Remembered panels by id, with the date of their last answer.
type Memory = BTreeMap<String, (CastDevice, u64)>;

/// Panels seen before, kept because a busy network (a school at 10 am) may not let their
/// answers through: ~/.local/state/sbcast/panels.tsv.
fn panels_file() -> std::path::PathBuf {
    crate::screenshare::trace::path().with_file_name("panels.tsv")
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The file's panels, minus those silent for `FORGET_AFTER`.
fn parse(text: &str, now: u64) -> Memory {
    text.lines()
        .filter_map(|line| CastDevice::from_line(line, now))
        .filter(|(_, seen)| now.saturating_sub(*seen) < FORGET_AFTER)
        .map(|(d, seen)| (d.id.clone(), (d, seen)))
        .collect()
}

fn load(now: u64) -> Memory {
    parse(&std::fs::read_to_string(panels_file()).unwrap_or_default(), now)
}

/// Panels seen on earlier runs; none when the file is missing or unreadable.
pub fn remembered() -> Vec<CastDevice> {
    load(now()).into_values().map(|(d, _)| d).collect()
}

/// Date `device`'s answer in `memory`. True when its line changed: new panel, new address
/// or name, or a date a day old.
fn update(memory: &mut Memory, device: &CastDevice, now: u64) -> bool {
    let fresh = |(old, seen): &(CastDevice, u64)| old.to_line(*seen) == device.to_line(*seen) && now.saturating_sub(*seen) < DATE_STEP;
    if memory.get(&device.id).is_some_and(fresh) {
        return false;
    }
    memory.insert(device.id.clone(), (device.clone(), now));
    true
}

/// `device` answered (mDNS, UDP search or ping): remember it, and drop the panels silent
/// for 3 weeks. A failure only costs the memory, so it is logged and skipped.
pub fn seen(device: &CastDevice) {
    let now = now();
    let mut memory = load(now);
    if update(&mut memory, device, now) {
        save(&memory);
    }
}

/// Drop `ids` from the panels file: the user asked to forget them. They come back on their
/// next answer.
pub fn forget(ids: &[String]) {
    let mut memory = load(now());
    memory.retain(|id, _| !ids.contains(id));
    save(&memory);
}

fn save(memory: &Memory) {
    let path = panels_file();
    let text: String = memory.values().map(|(d, seen)| d.to_line(*seen) + "\n").collect();
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

/// Send the UDP search to the local network; panels answer on the same port.
/// None when the port cannot be opened: the mDNS search still runs.
// ponytail: one broadcast to 255.255.255.255 leaves by the default route only; send it on each
// interface (getifaddrs) if the PC has panels on two networks.
fn udp_search() -> Option<UdpSocket> {
    let opened = (|| {
        let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)?;
        // Shared: the panels' own client, if it runs on the PC, holds the same port.
        socket.set_reuse_address(true)?;
        socket.set_broadcast(true)?;
        socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, SEARCH_PORT)).into())?;
        let socket: UdpSocket = socket.into();
        socket.set_nonblocking(true)?;
        socket.send_to(&search_message(), (Ipv4Addr::BROADCAST, SEARCH_PORT))?;
        std::io::Result::Ok(socket)
    })();
    opened.inspect_err(|e| tracing::warn!("UDP search not sent: {e}")).ok()
}

/// 50 bytes: the word's length (big-endian) at 12, the word at 20.
fn search_message() -> [u8; 50] {
    let mut message = [0u8; 50];
    message[12..16].copy_from_slice(&(SEARCH_WORD.len() as u32).to_be_bytes());
    message[20..20 + SEARCH_WORD.len()].copy_from_slice(SEARCH_WORD);
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
    let browsers = [mdns.browse(SERVICE_TYPE)?, mdns.browse(RTSP_SERVICE_TYPE)?];
    let search = udp_search();
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
        while let Some(Ok((len, SocketAddr::V4(from)))) = search.as_ref().map(|s| s.recv_from(&mut reply)) {
            if CastDevice::from_search_reply(&reply[..len], *from.ip()).is_some_and(&mut on_device) {
                stopped = true;
                break 'browse;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The next round's browse asks the network again and replays the cache.
    for ty in [SERVICE_TYPE, RTSP_SERVICE_TYPE] {
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
    fn names_an_rtsp_panel_after_its_room() {
        let ip = Ipv4Addr::new(10, 116, 7, 254);
        let device = CastDevice::rtsp_panel(&format!("COL-A11.{RTSP_SERVICE_TYPE}"), ip, 57395).unwrap();
        assert_eq!((device.name.as_str(), device.model.as_str(), device.id.as_str()), ("COL-A11", RTSP_PANEL, "rtsp:COL-A11"));
        assert!(CastDevice::rtsp_panel("COL-A11._sspcast._tcp.local.", ip, 57395).is_none());
    }

    #[test]
    fn forgets_panels_silent_for_three_weeks() {
        const DAY: u64 = 24 * 3600;
        let now = 1_800_000_000;
        let device = CastDevice::rtsp_panel(&format!("COL-A11.{RTSP_SERVICE_TYPE}"), Ipv4Addr::new(10, 116, 7, 254), 57395).unwrap();
        let line = device.to_line(now - 20 * DAY);
        let old = CastDevice { id: "old".into(), ..device.clone() }.to_line(now - 21 * DAY);
        let before_dates = "five\tSalle 104\tPanel\t10.0.0.8\t7385";
        let memory = parse(&[line.as_str(), &old, before_dates, "COL-A11\tbroken"].join("\n"), now);
        assert_eq!(memory.keys().collect::<Vec<_>>(), ["five", "rtsp:COL-A11"]);
        assert_eq!(memory["rtsp:COL-A11"].0.to_line(now - 20 * DAY), line);
        assert_eq!(memory["five"].1, now);
    }

    #[test]
    fn rewrites_a_date_once_a_day_and_any_change_at_once() {
        let now = 1_800_000_000;
        let device = CastDevice::rtsp_panel(&format!("COL-A11.{RTSP_SERVICE_TYPE}"), Ipv4Addr::new(10, 116, 7, 254), 57395).unwrap();
        let mut memory = Memory::new();
        assert!(update(&mut memory, &device, now));
        assert!(!update(&mut memory, &device, now + 3600));
        let moved = CastDevice { ip: Ipv4Addr::new(10, 116, 7, 9), ..device.clone() };
        assert!(update(&mut memory, &moved, now + 3600));
        assert!(update(&mut memory, &moved, now + 3600 + 24 * 3600));
    }

    #[test]
    fn reads_a_search_reply() {
        let ip = Ipv4Addr::new(10, 116, 11, 20);
        let device = CastDevice::from_search_reply(b"Model:COL-A22:8121:25123:56789:sn=0x103001ff", ip).unwrap();
        assert_eq!((device.id.as_str(), device.name.as_str(), device.ip, device.port), ("rtsp:COL-A22", "COL-A22", ip, 57395));
        assert!(CastDevice::from_search_reply(SEARCH_WORD, ip).is_none());
        let message = search_message();
        assert_eq!((message[15], &message[20..33]), (13, SEARCH_WORD));
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
