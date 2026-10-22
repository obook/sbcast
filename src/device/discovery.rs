//! discovery.rs
//! Find the interactive panels (service `_sspcast._tcp`) on the local network with mDNS.
//! Used by `sbcast list`, by `--device` and by the menu's list.
//!
//! Author: O. Booklage
//! Date: October 2026
//! Licence: MIT

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use mdns_sd::{RecvTimeoutError, ResolvedService, ServiceDaemon, ServiceEvent};

/// Unverified on a real panel.
const SERVICE_TYPE: &str = "_sspcast._tcp.local.";

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
        let txt = |key: &str| info.get_property_val_str(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Self::from_txt(txt, info.get_addresses_v4().into_iter().next(), info.get_port())
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

/// Call `on_device` for each resolved panel until it returns true or `timeout` expires.
pub fn browse(timeout: Duration, mut on_device: impl FnMut(CastDevice) -> bool) -> anyhow::Result<()> {
    let mdns = ServiceDaemon::new()?;
    let browser = mdns.browse(SERVICE_TYPE)?;
    let deadline = Instant::now() + timeout;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match browser.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                tracing::debug!(?info, "resolved");
                if CastDevice::from_service(&info).is_some_and(&mut on_device) {
                    break;
                }
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = mdns.shutdown();
    Ok(())
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
