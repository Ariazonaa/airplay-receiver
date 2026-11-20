//! Port von test_adapters.cpp.
use airplay::discovery::adapters::{
    NetAdapter, adapter_changed, enumerate_adapters, looks_virtual, pick_adapter,
};

const ETHERNET: u32 = 6; // IF_TYPE_ETHERNET_CSMACD
const WIFI: u32 = 71; // IF_TYPE_IEEE80211
const PROP_VIRTUAL: u32 = 53; // IF_TYPE_PROP_VIRTUAL

fn make(name: &str, desc: &str, if_type: u32, ip: &str, idx: u32, up: bool) -> NetAdapter {
    NetAdapter {
        name: name.to_string(),
        description: desc.to_string(),
        if_index: idx,
        if_type,
        mac: [0; 6],
        ipv4: if ip.is_empty() {
            Vec::new()
        } else {
            vec![ip.to_string()]
        },
        up,
    }
}

/// Nachbildung der Adapter dieses PCs (Stand 2026-11-19).
fn this_pc() -> Vec<NetAdapter> {
    vec![
        make(
            "wg0-client-PC",
            "WireGuard Tunnel",
            PROP_VIRTUAL,
            "10.8.0.2",
            11,
            true,
        ),
        make(
            "Mullvad",
            "Mullvad Tunnel",
            PROP_VIRTUAL,
            "10.64.0.5",
            12,
            true,
        ),
        make(
            "pbs",
            "WireGuard Tunnel #2",
            PROP_VIRTUAL,
            "10.9.0.2",
            13,
            true,
        ),
        make(
            "vEthernet (Default Switch)",
            "Hyper-V Virtual Ethernet Adapter",
            ETHERNET,
            "172.20.0.1",
            14,
            true,
        ),
        make(
            "Ethernet",
            "Realtek(R) PCI(e) Ethernet Controller",
            ETHERNET,
            "192.168.178.42",
            15,
            true,
        ),
    ]
}

#[test]
fn auto_picks_physical_ethernet() {
    let a = pick_adapter(&this_pc(), "auto").expect("Adapter");
    assert_eq!(a.name, "Ethernet");
    assert_eq!(a.if_index, 15);
}

#[test]
fn virtual_adapters_detected() {
    let list = this_pc();
    assert!(looks_virtual(&list[0]));
    assert!(looks_virtual(&list[1]));
    assert!(looks_virtual(&list[2]));
    assert!(looks_virtual(&list[3]));
    assert!(!looks_virtual(&list[4]));
    assert!(looks_virtual(&make(
        "Loopback",
        "Software Loopback Interface 1",
        24,
        "127.0.0.1",
        1,
        true
    )));
    assert!(looks_virtual(&make(
        "Ethernet 3",
        "TAP-Windows Adapter V9",
        ETHERNET,
        "10.1.1.1",
        20,
        true
    )));
    assert!(looks_virtual(&make(
        "vEthernet (WSL)",
        "Hyper-V Virtual Ethernet Adapter #2",
        ETHERNET,
        "172.28.0.1",
        21,
        true
    )));
}

#[test]
fn explicit_name_wins_even_if_virtual() {
    let a = pick_adapter(&this_pc(), "Mullvad").expect("Adapter");
    assert_eq!(a.name, "Mullvad");
}

#[test]
fn ethernet_preferred_over_wifi() {
    let list = vec![
        make(
            "WLAN",
            "Intel Wi-Fi 6E AX211",
            WIFI,
            "192.168.178.50",
            3,
            true,
        ),
        make(
            "Ethernet",
            "Intel I225-V",
            ETHERNET,
            "192.168.178.51",
            4,
            true,
        ),
    ];
    assert_eq!(pick_adapter(&list, "auto").unwrap().name, "Ethernet");
}

#[test]
fn down_adapter_skipped() {
    let list = vec![
        make(
            "Ethernet",
            "Intel I225-V",
            ETHERNET,
            "192.168.178.51",
            4,
            false,
        ),
        make(
            "WLAN",
            "Intel Wi-Fi 6E AX211",
            WIFI,
            "192.168.178.50",
            3,
            true,
        ),
    ];
    assert_eq!(pick_adapter(&list, "auto").unwrap().name, "WLAN");
}

#[test]
fn adapter_without_ipv4_skipped() {
    let list = vec![
        make("Ethernet", "Intel I225-V", ETHERNET, "", 4, true),
        make(
            "WLAN",
            "Intel Wi-Fi 6E AX211",
            WIFI,
            "192.168.178.50",
            3,
            true,
        ),
    ];
    assert_eq!(pick_adapter(&list, "auto").unwrap().name, "WLAN");
}

#[test]
fn no_candidate_returns_nullopt() {
    let list = vec![make(
        "Mullvad",
        "Mullvad Tunnel",
        PROP_VIRTUAL,
        "10.64.0.5",
        12,
        true,
    )];
    assert!(pick_adapter(&list, "auto").is_none());
    assert!(pick_adapter(&this_pc(), "Gibtsnicht").is_none());
}

#[test]
fn adapter_change_detection() {
    let eth = make("Ethernet", "Realtek", ETHERNET, "192.168.178.42", 15, true);
    assert!(!adapter_changed(&eth, Some(&eth)));
    let mut new_ip = eth.clone();
    new_ip.ipv4 = vec!["192.168.178.77".to_string()];
    assert!(adapter_changed(&eth, Some(&new_ip))); // DHCP hat neue Adresse vergeben
    let other = make("WLAN", "Intel", WIFI, "192.168.178.50", 3, true);
    assert!(adapter_changed(&eth, Some(&other))); // anderer Adapter
    assert!(adapter_changed(&eth, None)); // Netz weg
    let mut extra_ip = eth.clone();
    extra_ip.ipv4.push("10.0.0.5".to_string());
    assert!(!adapter_changed(&eth, Some(&extra_ip))); // erste Adresse gleich → bleibt
}

#[test]
fn enumerate_real_adapters_does_not_crash() {
    for a in enumerate_adapters() {
        assert!(!a.name.is_empty());
    }
}
