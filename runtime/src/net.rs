//! Host network state, for the screen shown before the player is up.
//!
//! Volumio already publishes network state, and this consumes that rather
//! than independently interrogating the kernel.
//!
//! `volumio-os/volumio/bin/wireless.js` maintains three files as a
//! notification channel, and `wireless.service` is ordered
//! `Before=volumio.service`, so they are live during exactly the window this
//! display exists to fill:
//!
//! ```text
//! /tmp/networkstatus   ap | hotspot | offline, mtime touched on every change
//! /data/wlan0status    connected | hotspot | disconnected
//! /data/eth0status     connected | disconnected
//! ```
//!
//! `refreshNetworkStatusFile()` in that daemon exists solely to touch the
//! mtime so watchers fire. Polling `getifaddrs` on a timer alongside it would
//! be second-guessing the component that actually manages the network.
//!
//! Addresses themselves still come from the kernel, because the status files
//! say what mode the network is in, not what address was assigned. They are
//! read when the signal fires, not on a timer.

use std::net::{IpAddr, Ipv6Addr};
use std::path::Path;
use std::time::SystemTime;

/// Where `wireless.js` publishes its state.
const SIGNAL: &str = "/tmp/networkstatus";
const WLAN_STATUS: &str = "/data/wlan0status";
const HOSTAPD_CONF: &str = "/etc/hostapd/hostapd.conf";

/// Volumio's access point address. Hardcoded in `wireless.js` as
/// `ifconfig wlan0 192.168.211.1 up`, and treated as "not connected" by the
/// backend's own `network_monitor.sh`.
const HOTSPOT_ADDR: &str = "192.168.211.1";

/// Fallback SSID, matching the backend's own default for `hotspot_name`.
const DEFAULT_SSID: &str = "Volumio";

/// How often addresses are re-read when the notifier file is absent.
///
/// Only reached on an image without `wireless.js`, or before it has run once.
const FALLBACK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// One address, with the interface it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// Interface name, e.g. `eth0`.
    pub iface: String,
    /// The address, formatted for display.
    pub addr: String,
}

/// What the status screen should show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetState {
    /// No usable address yet. The board is alive and looking.
    Waiting,
    /// Access point mode: join this network, then open this address.
    Hotspot {
        /// SSID being broadcast.
        ssid: String,
        /// The access point's own address.
        addr: String,
    },
    /// Connected, with one or more usable addresses.
    Connected(Vec<Address>),
}

/// Hostname and network state, as displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    /// Hostname with the mDNS suffix, because that is what people type.
    pub hostname: String,
    /// Current network state.
    pub state: NetState,
}

impl Default for HostInfo {
    fn default() -> Self {
        Self {
            hostname: String::new(),
            state: NetState::Waiting,
        }
    }
}

/// Reads host state, and knows when it is worth re-reading.
pub struct NetMonitor {
    /// mtime of the notifier file at the last read.
    seen: Option<SystemTime>,
    /// When the fallback timer last fired, used only when the notifier file
    /// is absent.
    last_poll: std::time::Instant,
}

impl Default for NetMonitor {
    fn default() -> Self {
        Self {
            seen: None,
            // Far enough back that the first call always reads.
            last_poll: std::time::Instant::now() - FALLBACK_INTERVAL,
        }
    }
}

impl NetMonitor {
    /// Return fresh state if something has changed, otherwise `None`.
    ///
    /// Cheap to call on every loop tick: in the common case it is one `stat`.
    pub fn poll(&mut self) -> Option<HostInfo> {
        match modified(SIGNAL) {
            Some(mtime) => {
                if self.seen == Some(mtime) {
                    return None;
                }
                self.seen = Some(mtime);
            }
            // No notifier file: an older image, or wireless.js has not run
            // yet. Degrade to a timer rather than showing nothing.
            None => {
                if self.last_poll.elapsed() < FALLBACK_INTERVAL {
                    return None;
                }
                self.last_poll = std::time::Instant::now();
            }
        }

        Some(read())
    }
}

fn modified(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Read the current host state.
fn read() -> HostInfo {
    HostInfo {
        hostname: hostname(),
        state: state(),
    }
}

/// Hostname with the mDNS suffix.
///
/// avahi is running on every Volumio image, and `volumio.local` is what a
/// person will actually type. The bare hostname is not useful off the box.
fn hostname() -> String {
    let name = std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if name.is_empty() {
        String::new()
    } else {
        format!("{name}.local")
    }
}

fn state() -> NetState {
    if in_hotspot_mode() {
        return NetState::Hotspot {
            ssid: hotspot_ssid(),
            addr: HOTSPOT_ADDR.to_string(),
        };
    }

    let addrs = addresses();
    if addrs.is_empty() {
        NetState::Waiting
    } else {
        NetState::Connected(addrs)
    }
}

/// True when `wireless.js` reports access point mode.
///
/// Taken from the daemon's own status file rather than inferred from the
/// address, because the file is its statement of intent and the address is a
/// consequence. The address is checked as a fallback for the case where the
/// file has not been written yet.
fn in_hotspot_mode() -> bool {
    if let Ok(s) = std::fs::read_to_string(WLAN_STATUS) {
        return s.trim() == "hotspot";
    }
    addresses().iter().any(|a| a.addr == HOTSPOT_ADDR)
}

/// SSID from the file hostapd actually loads.
///
/// `rebuildHotspotConfig()` in the backend's network plugin writes
/// `ssid=<hotspot_name>` here, defaulting to `Volumio`. The backend is not
/// running during the window this display covers, but that does not matter:
/// hostapd is already running with whatever the file says, so the file and
/// the broadcast agree.
fn hotspot_ssid() -> String {
    std::fs::read_to_string(HOSTAPD_CONF)
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.trim().strip_prefix("ssid=").map(str::to_string))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SSID.to_string())
}

/// Usable addresses, ordered as someone reading the panel would want them.
fn addresses() -> Vec<Address> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };

    let mut out: Vec<(bool, bool, Address)> = ifaces
        .into_iter()
        .filter_map(|i| {
            let ip = i.addr.ip();
            if !usable(&ip) {
                return None;
            }
            Some((
                !is_wireless(&i.name), // wired sorts first
                ip.is_ipv6(),          // v4 sorts first
                Address {
                    iface: i.name,
                    addr: ip.to_string(),
                },
            ))
        })
        .collect();

    // Wired before wireless, v4 before v6, then by interface name so the
    // order is stable across reads and the panel does not reshuffle.
    out.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.iface.cmp(&b.2.iface))
            .then(a.2.addr.cmp(&b.2.addr))
    });
    out.dedup_by(|a, b| a.2 == b.2);
    out.into_iter().map(|(_, _, a)| a).collect()
}

/// True for an interface the kernel considers wireless.
///
/// Checked against sysfs rather than by name prefix: `wlan0` is conventional
/// but not guaranteed, and USB adapters on Volumio are renamed by a udev rule.
fn is_wireless(name: &str) -> bool {
    Path::new(&format!("/sys/class/net/{name}/wireless")).exists()
}

/// True for an address someone could actually connect to.
///
/// The IPv6 classification is hand-rolled because the standard library's
/// `is_unique_local` and `is_unicast_link_local` are still unstable. The
/// prefixes are from RFC 4193 and RFC 4291 and do not move.
fn usable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            // Link-local means DHCP failed. Showing 169.254.x.x implies a
            // working address when there is not one, which is worse than
            // showing nothing.
            !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified() && !v4.is_multicast()
        }
        IpAddr::V6(v6) => {
            !v6.is_loopback()
                && !v6.is_unspecified()
                && !v6.is_multicast()
                // fe80::/10, link-local. Needs a scope identifier to be
                // usable and is meaningless typed into a browser.
                && !is_v6_link_local(v6)
        }
    }
}

/// fe80::/10.
fn is_v6_link_local(a: &Ipv6Addr) -> bool {
    let o = a.octets();
    o[0] == 0xfe && (o[1] & 0xc0) == 0x80
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::str::FromStr;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from_str(s).unwrap())
    }
    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(Ipv6Addr::from_str(s).unwrap())
    }

    #[test]
    fn keeps_routable_v4() {
        assert!(usable(&v4("192.168.30.205")));
        assert!(usable(&v4("10.0.0.7")));
        assert!(usable(&v4("81.2.69.142")));
    }

    #[test]
    fn drops_v4_nobody_can_use() {
        assert!(!usable(&v4("127.0.0.1")));
        assert!(!usable(&v4("169.254.11.9")), "link-local means DHCP failed");
        assert!(!usable(&v4("0.0.0.0")));
    }

    #[test]
    fn keeps_global_and_unique_local_v6() {
        assert!(usable(&v6("2001:db8::1")));
        assert!(
            usable(&v6("fd00:1234::5")),
            "unique local is routable on a LAN"
        );
    }

    #[test]
    fn drops_v6_nobody_can_use() {
        assert!(!usable(&v6("::1")));
        assert!(!usable(&v6("::")));
        assert!(!usable(&v6("fe80::1")), "link-local needs a scope id");
        assert!(!usable(&v6("ff02::1")));
    }

    #[test]
    fn identifies_v6_link_local_prefix() {
        // fe80::/10 covers fe80 through febf.
        assert!(is_v6_link_local(&Ipv6Addr::from_str("fe80::1").unwrap()));
        assert!(is_v6_link_local(&Ipv6Addr::from_str("febf::1").unwrap()));
        assert!(!is_v6_link_local(&Ipv6Addr::from_str("fec0::1").unwrap()));
        assert!(!is_v6_link_local(&Ipv6Addr::from_str("fd00::1").unwrap()));
    }
}
