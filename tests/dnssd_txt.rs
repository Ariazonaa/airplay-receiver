//! Port von test_dnssd_txt.cpp.
use std::collections::HashMap;
use std::ffi::{CString, c_char};

use airplay::discovery::dnssd::*;
use airplay::ffi::uxplay::dnssd_t;

const MAC: [u8; 6] = [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F];
const PK: &str = "b07727d6f6cd6e08b58ede525ec3cdeaa252ad9f683feb212ef8a205246554e7";

/// TXT-Wire-Format: je Eintrag 1 Längenbyte + "key=value".
fn parse_txt(data: *const c_char, len: i32) -> HashMap<String, String> {
    assert!(!data.is_null());
    // SAFETY: dnssd_get_*_txt liefert `len` gültige Bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, len as usize) };
    let mut out = HashMap::new();
    let mut i = 0;
    while i < bytes.len() {
        let n = bytes[i] as usize;
        i += 1;
        assert!(i + n <= bytes.len());
        let entry = String::from_utf8(bytes[i..i + n].to_vec()).unwrap();
        i += n;
        match entry.split_once('=') {
            Some((k, v)) => out.insert(k.to_string(), v.to_string()),
            None => out.insert(entry, String::new()),
        };
    }
    out
}

fn make_dnssd(name: &str) -> *mut dnssd_t {
    let mut err = -1;
    // SAFETY: Zeiger zeigen auf gültige lokale Daten.
    let d = unsafe {
        dnssd_init(
            name.as_ptr() as *const c_char,
            name.len() as i32,
            MAC.as_ptr() as *const c_char,
            6,
            &mut err,
            0,
        )
    };
    assert!(!d.is_null());
    assert_eq!(err, 0);
    let pk = CString::new(PK).unwrap();
    // SAFETY: d ist gültig, pk nullterminiert.
    unsafe { dnssd_set_pk(d, pk.as_ptr() as *mut c_char) };
    d
}

fn airplay_txt(d: *mut dnssd_t) -> HashMap<String, String> {
    let mut len = 0;
    // SAFETY: d ist gültig.
    let txt = unsafe { dnssd_get_airplay_txt(d, &mut len) };
    parse_txt(txt, len)
}

#[test]
fn airplay_txt_has_required_keys() {
    let d = make_dnssd("Wohnzimmer");
    let kv = airplay_txt(d);
    assert_eq!(kv["deviceid"], "0a:0b:0c:0d:0e:0f"); // utils_hwaddr_airplay schreibt klein
    assert_eq!(kv["features"], "0x5A7FFEE6,0x0");
    assert_eq!(kv["flags"], "0x4");
    assert_eq!(kv["pw"], "false");
    assert_eq!(kv["pk"], PK);
    assert_eq!(kv["pi"], "2e388006-13ba-4041-9a67-25dd4a43d536");
    assert_eq!(kv["vv"], "2");
    assert!(kv.contains_key("model"));
    assert!(kv.contains_key("srcvers"));
    // SAFETY: d stammt aus dnssd_init.
    unsafe { dnssd_destroy(d) };
}

#[test]
fn raop_txt_has_required_keys() {
    let d = make_dnssd("Wohnzimmer");
    let mut len = 0;
    // SAFETY: d ist gültig.
    let txt = unsafe { dnssd_get_raop_txt(d, &mut len) };
    let kv = parse_txt(txt, len);
    for (k, v) in [
        ("ch", "2"),
        ("cn", "0,1,2,3"),
        ("da", "true"),
        ("et", "0,3,5"),
        ("vv", "2"),
        ("ft", "0x5A7FFEE6,0x0"),
        ("md", "0,1,2"),
        ("rhd", "5.6.0.0"),
        ("pw", "false"),
        ("sf", "0x4"),
        ("sr", "44100"),
        ("ss", "16"),
        ("sv", "false"),
        ("tp", "UDP"),
        ("txtvers", "1"),
        ("vn", "65537"),
        ("pk", PK),
    ] {
        assert_eq!(kv[k], v, "Schlüssel {k}");
    }
    assert!(kv.contains_key("am"));
    assert!(kv.contains_key("vs"));
    // SAFETY: d stammt aus dnssd_init.
    unsafe { dnssd_destroy(d) };
}

#[test]
fn features_bits_roundtrip() {
    let d = make_dnssd("X");
    // SAFETY: d ist gültig.
    unsafe {
        dnssd_set_airplay_features(d, 30, 1);
        assert_eq!((dnssd_get_airplay_features(d) >> 30) & 1, 1);
        dnssd_set_airplay_features(d, 40, 1);
        assert_eq!((dnssd_get_airplay_features(d) >> 40) & 1, 1);
        dnssd_set_airplay_features(d, 1, 0);
        assert_eq!((dnssd_get_airplay_features(d) >> 1) & 1, 0);
    }
    assert_eq!(airplay_txt(d)["features"], "0x5A7FFEE4,0x100");
    // SAFETY: d stammt aus dnssd_init.
    unsafe { dnssd_destroy(d) };
}

#[test]
fn utf8_name_in_txt_and_instance_name() {
    let name = "Küche Ü";
    let d = make_dnssd(name);
    let mut len = 0;
    // SAFETY: d ist gültig; der Name ist `len` Bytes lang.
    let got = unsafe {
        let p = dnssd_get_name(d, &mut len);
        std::slice::from_raw_parts(p as *const u8, len as usize)
    };
    assert_eq!(got, name.as_bytes());
    assert_eq!(raop_instance_name(&MAC, name), "0A0B0C0D0E0F@Küche Ü");
    assert_eq!(airplay_instance_name(name), "Küche Ü");
    // SAFETY: d stammt aus dnssd_init.
    unsafe { dnssd_destroy(d) };
}

#[test]
fn utf8_truncation_never_splits_a_character() {
    let ue = "ü".repeat(40); // 80 Bytes
    let cut = fit_utf8(&ue, 50);
    assert_eq!(cut.len(), 50);
    assert_eq!(cut, &ue[..50]);
    let odd = format!("a{ue}"); // ein Byte versetzt
    let cut2 = fit_utf8(&odd, 50);
    assert_eq!(cut2.len(), 49); // 'a' + 24 × ü, das halbe 25. ü fällt weg
    assert_eq!(fit_utf8("kurz", 50), "kurz");
}

#[test]
fn long_names_fit_into_63_byte_dns_labels() {
    let ue = "ü".repeat(40);
    let raop = raop_instance_name(&MAC, &ue);
    let ap = airplay_instance_name(&ue);
    assert!(raop.len() <= 63);
    assert!(ap.len() <= 63);
    assert!(raop.starts_with("0A0B0C0D0E0F@"));
}

#[test]
fn dots_in_instance_names_are_escaped() {
    assert_eq!(
        service_fqdn("Mr.PC", "_airplay._tcp"),
        "Mr\\.PC._airplay._tcp.local"
    );
    assert_eq!(
        service_fqdn("a\\b", "_raop._tcp"),
        "a\\\\b._raop._tcp.local"
    );
}

fn raw_txt(
    get: unsafe extern "C" fn(*mut dnssd_t, *mut i32) -> *const c_char,
    d: *mut dnssd_t,
) -> (*const c_char, Vec<u8>) {
    let mut len = 0;
    // SAFETY: d ist gültig; der Getter liefert `len` lesbare Bytes.
    unsafe {
        let p = get(d, &mut len);
        assert!(!p.is_null());
        (
            p,
            std::slice::from_raw_parts(p as *const u8, len as usize).to_vec(),
        )
    }
}

/// Die Getter bauen nichts neu: gleicher Puffer bei jedem Aufruf (UxPlays httpd-Thread liest
/// ihn ohne Sperre). Ein geänderter Schlüssel ergibt einen neuen Puffer; der alte bleibt bis
/// dnssd_destroy lesbar und unverändert.
#[test]
fn txt_getters_return_stored_buffer() {
    let d = make_dnssd("Wohnzimmer");
    for get in [dnssd_get_raop_txt, dnssd_get_airplay_txt] {
        let (p1, b1) = raw_txt(get, d);
        let (p2, b2) = raw_txt(get, d);
        assert_eq!(p1, p2, "Getter liefert jedes Mal einen neuen Puffer");
        assert_eq!(b1, b2);
        assert_eq!(parse_txt(p1, b1.len() as i32)["pk"], PK);
    }
    let (old_raop, old_raop_bytes) = raw_txt(dnssd_get_raop_txt, d);
    let (old_ap, old_ap_bytes) = raw_txt(dnssd_get_airplay_txt, d);
    let pk2 = CString::new("ff".repeat(32)).unwrap();
    // SAFETY: d ist gültig, pk2 nullterminiert.
    unsafe { dnssd_set_pk(d, pk2.as_ptr() as *mut c_char) };
    let (_, new_raop) = raw_txt(dnssd_get_raop_txt, d);
    let (_, new_ap) = raw_txt(dnssd_get_airplay_txt, d);
    assert_eq!(
        parse_txt(new_raop.as_ptr() as *const c_char, new_raop.len() as i32)["pk"],
        "ff".repeat(32)
    );
    assert_eq!(
        parse_txt(new_ap.as_ptr() as *const c_char, new_ap.len() as i32)["pk"],
        "ff".repeat(32)
    );
    // SAFETY: laut Vertrag bleiben ausgegebene Puffer bis dnssd_destroy gültig.
    unsafe {
        assert_eq!(
            std::slice::from_raw_parts(old_raop as *const u8, old_raop_bytes.len()),
            &old_raop_bytes[..]
        );
        assert_eq!(
            std::slice::from_raw_parts(old_ap as *const u8, old_ap_bytes.len()),
            &old_ap_bytes[..]
        );
    }
    // SAFETY: d stammt aus dnssd_init.
    unsafe { dnssd_destroy(d) };
}

/// Ein Thread liest die TXT-Puffer (wie UxPlays httpd bei GET /info), während der andere
/// Schlüssel und Feature-Bits ändert: jeder gelesene Puffer bleibt ein gültiger TXT-Eintrag.
#[test]
fn txt_getters_stay_valid_while_another_thread_updates() {
    let d = make_dnssd("Küche");
    let addr = d as usize;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let d = addr as *mut dnssd_t;
            let mut reads = 0u32;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) || reads < 1000 {
                for get in [dnssd_get_raop_txt, dnssd_get_airplay_txt] {
                    let (p, b) = raw_txt(get, d);
                    let kv = parse_txt(p, b.len() as i32);
                    assert_eq!(kv["pk"].len(), 64);
                }
                reads += 1;
            }
        })
    };
    let pks = [
        CString::new(PK).unwrap(),
        CString::new("ab".repeat(32)).unwrap(),
    ];
    for i in 0..2000 {
        // SAFETY: d ist gültig, die Schlüssel sind nullterminiert.
        unsafe {
            dnssd_set_pk(d, pks[i % 2].as_ptr() as *mut c_char);
            dnssd_set_airplay_features(d, 40, (i % 2) as i32);
        }
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    reader.join().unwrap();
    // SAFETY: d stammt aus dnssd_init; der Lese-Thread ist beendet.
    unsafe { dnssd_destroy(d) };
}
