//! Port von test_config.cpp.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use airplay::app::config::*;

fn fresh_temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join("airplay-tests").join(format!(
        "{}_{}_{}",
        name,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(p: &Path, s: &str) {
    std::fs::write(p, s).unwrap();
}

#[test]
fn missing_file_yields_defaults() {
    let dir = fresh_temp_dir("cfg");
    let c = load_config(&dir.join("nope.json"));
    assert_eq!(c.mode, Mode::LowLatency);
    assert_eq!(c.smooth_delay_ms, 200);
    assert!(!c.allow_h265);
    assert!(!c.stats_overlay);
    assert_eq!(c.adapter, "auto");
    assert_eq!(c.receiver_name, default_config().receiver_name);
    assert!(!c.receiver_name.is_empty());
}

#[test]
fn roundtrip() {
    let dir = fresh_temp_dir("cfg");
    let file = dir.join("sub").join("config.json");
    let mut c = default_config();
    c.receiver_name = "Küchen-PC 📺".to_string();
    c.mode = Mode::Smooth;
    c.smooth_delay_ms = 300;
    c.allow_h265 = true;
    c.stats_overlay = true;
    c.adapter = "Ethernet".to_string();
    c.screenshot_dir = dir.join("bilder");
    c.record_dir = dir.join("videos");
    assert!(save_config(&c, &file));
    let r = load_config(&file);
    assert_eq!(r.receiver_name, c.receiver_name);
    assert_eq!(r.mode, Mode::Smooth);
    assert_eq!(r.smooth_delay_ms, 300);
    assert!(r.allow_h265);
    assert!(r.stats_overlay);
    assert_eq!(r.adapter, "Ethernet");
    assert_eq!(r.screenshot_dir, c.screenshot_dir);
    assert_eq!(r.record_dir, c.record_dir);
}

#[test]
fn corrupt_file_yields_defaults() {
    let dir = fresh_temp_dir("cfg");
    write_file(&dir.join("c.json"), "{nope");
    let c = load_config(&dir.join("c.json"));
    assert_eq!(c.mode, Mode::LowLatency);
    assert_eq!(c.smooth_delay_ms, 200);
}

#[test]
fn delay_is_clamped() {
    let dir = fresh_temp_dir("cfg");
    write_file(&dir.join("a.json"), r#"{"smoothDelayMs": 20}"#);
    assert_eq!(load_config(&dir.join("a.json")).smooth_delay_ms, 100);
    write_file(&dir.join("b.json"), r#"{"smoothDelayMs": 9000}"#);
    assert_eq!(load_config(&dir.join("b.json")).smooth_delay_ms, 500);
}

#[test]
fn empty_name_falls_back() {
    let dir = fresh_temp_dir("cfg");
    write_file(&dir.join("c.json"), r#"{"receiverName": "   "}"#);
    assert_eq!(
        load_config(&dir.join("c.json")).receiver_name,
        default_config().receiver_name
    );
}

#[test]
fn unknown_mode_string_falls_back() {
    let dir = fresh_temp_dir("cfg");
    write_file(
        &dir.join("c.json"),
        r#"{"mode": "turbo", "allowH265": true}"#,
    );
    let c = load_config(&dir.join("c.json"));
    assert_eq!(c.mode, Mode::LowLatency);
    assert!(c.allow_h265);
}

#[test]
fn wrong_value_types_fall_back_per_field() {
    let dir = fresh_temp_dir("cfg");
    write_file(
        &dir.join("c.json"),
        r#"{"smoothDelayMs": "viel", "mode": "smooth"}"#,
    );
    let c = load_config(&dir.join("c.json"));
    assert_eq!(c.smooth_delay_ms, 200);
    assert_eq!(c.mode, Mode::Smooth);
}

#[test]
fn language_roundtrip() {
    let dir = fresh_temp_dir("cfg");
    let mut c = default_config();
    assert_eq!(c.language, "auto");
    c.language = "en".to_string();
    assert!(save_config(&c, &dir.join("c.json")));
    assert_eq!(load_config(&dir.join("c.json")).language, "en");
}

#[test]
fn invalid_language_falls_back_to_auto() {
    let dir = fresh_temp_dir("cfg");
    write_file(&dir.join("c.json"), r#"{"language": "klingonisch"}"#);
    assert_eq!(load_config(&dir.join("c.json")).language, "auto");
}

#[test]
fn config_roundtrip_in_umlaut_dir() {
    let dir = fresh_temp_dir("cfg").join("Ä ö");
    let file = dir.join("config.json");
    let mut c = default_config();
    c.receiver_name = "Wohnzimmer-TV".to_string();
    c.screenshot_dir = dir.join("Bilder Ü");
    c.record_dir = dir.join("Videos ß");
    assert!(save_config(&c, &file));
    assert_eq!(load_config(&file), c);
}
