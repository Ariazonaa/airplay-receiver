//! Netzwerkadapter auflisten und den passenden für die Ankündigung wählen.

use std::net::Ipv4Addr;

use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

use crate::platform::win::paths::from_wide;

const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
const IF_TYPE_PPP: u32 = 23;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_TYPE_PROP_VIRTUAL: u32 = 53;
const IF_TYPE_IEEE80211: u32 = 71;
const IF_TYPE_TUNNEL: u32 = 131;

const VIRTUAL_MARKERS: [&str; 12] = [
    "hyper-v",
    "vethernet",
    "wsl",
    "wireguard",
    "mullvad",
    "tap-windows",
    "wintun",
    "vpn",
    "virtualbox",
    "vmware",
    "tailscale",
    "zerotier",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetAdapter {
    /// FriendlyName
    pub name: String,
    pub description: String,
    pub if_index: u32,
    /// IF_TYPE_*
    pub if_type: u32,
    pub mac: [u8; 6],
    pub ipv4: Vec<String>,
    pub up: bool,
}

fn type_rank(if_type: u32) -> u32 {
    match if_type {
        IF_TYPE_ETHERNET_CSMACD => 0,
        IF_TYPE_IEEE80211 => 1,
        _ => 2,
    }
}

/// Alle IPv4-fähigen Adapter (GetAdaptersAddresses).
pub fn enumerate_adapters() -> Vec<NetAdapter> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    // u64 statt u8, damit der Puffer für IP_ADAPTER_ADDRESSES ausgerichtet ist.
    let mut buf: Vec<u64> = Vec::new();
    let mut rc = ERROR_BUFFER_OVERFLOW.0;
    for _ in 0..3 {
        if rc != ERROR_BUFFER_OVERFLOW.0 {
            break;
        }
        buf.resize((size as usize).div_ceil(8), 0);
        // SAFETY: der Puffer ist mindestens `size` Bytes groß und ausgerichtet.
        rc = unsafe {
            GetAdaptersAddresses(
                AF_INET.0 as u32,
                flags,
                None,
                Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut size,
            )
        };
    }
    if rc != NO_ERROR.0 {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    // SAFETY: GetAdaptersAddresses hat im Puffer eine gültige verkettete Liste aufgebaut,
    // der Puffer lebt während der ganzen Schleife.
    unsafe {
        while !cur.is_null() {
            let a = &*cur;
            cur = a.Next;
            let mut n = NetAdapter {
                name: if a.FriendlyName.is_null() {
                    String::new()
                } else {
                    from_wide(a.FriendlyName.as_wide())
                },
                description: if a.Description.is_null() {
                    String::new()
                } else {
                    from_wide(a.Description.as_wide())
                },
                if_index: a.Anonymous1.Anonymous.IfIndex,
                if_type: a.IfType,
                up: a.OperStatus == IfOperStatusUp,
                ..Default::default()
            };
            let mac_len = (a.PhysicalAddressLength as usize).min(6);
            n.mac[..mac_len].copy_from_slice(&a.PhysicalAddress[..mac_len]);
            let mut u = a.FirstUnicastAddress;
            while !u.is_null() {
                let ua = &*u;
                u = ua.Next;
                let sa = ua.Address.lpSockaddr;
                if sa.is_null() || (*sa).sa_family != AF_INET {
                    continue;
                }
                let sin = &*(sa as *const SOCKADDR_IN);
                let octets = sin.sin_addr.S_un.S_addr.to_ne_bytes();
                n.ipv4.push(Ipv4Addr::from(octets).to_string());
            }
            if n.name.is_empty() {
                continue;
            }
            result.push(n);
        }
    }
    result
}

/// Loopback, Tunnel/virtuelle Typen oder bekannte VPN-/VM-Namen.
pub fn looks_virtual(a: &NetAdapter) -> bool {
    if matches!(
        a.if_type,
        IF_TYPE_SOFTWARE_LOOPBACK | IF_TYPE_PROP_VIRTUAL | IF_TYPE_TUNNEL | IF_TYPE_PPP
    ) {
        return true;
    }
    let text = format!("{} {}", a.name, a.description).to_ascii_lowercase();
    VIRTUAL_MARKERS.iter().any(|m| text.contains(m))
}

/// "auto" → erster aktiver, nicht virtueller Adapter mit IPv4, Ethernet vor WLAN;
/// sonst exakter Namenstreffer. Kein Treffer → `None`.
pub fn pick_adapter(list: &[NetAdapter], setting: &str) -> Option<NetAdapter> {
    if setting != "auto" {
        return list.iter().find(|a| a.name == setting).cloned();
    }
    let mut best: Option<&NetAdapter> = None;
    for a in list {
        if !a.up || a.ipv4.is_empty() || looks_virtual(a) {
            continue;
        }
        if best.is_none_or(|b| type_rank(a.if_type) < type_rank(b.if_type)) {
            best = Some(a);
        }
    }
    best.cloned()
}

/// Muss der Empfänger neu angekündigt werden? (anderer Adapter, neue erste IPv4, Netz weg)
pub fn adapter_changed(running: &NetAdapter, now: Option<&NetAdapter>) -> bool {
    let Some(now) = now else {
        return true;
    };
    if now.if_index != running.if_index {
        return true;
    }
    let first = |a: &NetAdapter| a.ipv4.first().cloned().unwrap_or_default();
    first(running) != first(now)
}
