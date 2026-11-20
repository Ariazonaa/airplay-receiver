//! Port von test_log.cpp. Eigener Testprozess, weil das Log global ist.
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use airplay::app::log::{LogLevel, log_init, log_shutdown};
use airplay::log_msg;

static SERIAL: Mutex<()> = Mutex::new(());

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

#[test]
fn log_writes_formatted_line() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fresh_temp_dir("log");
    log_init(&dir);
    log_msg!(LogLevel::Info, "hallo {}", 42);
    log_shutdown();
    let text = std::fs::read_to_string(dir.join("airplay.log")).unwrap();
    let line = text.lines().next().unwrap();
    assert!(line.contains("[INFO] hallo 42"), "{line}");
    assert!(line.len() > 24); // Zeitstempel davor
}

#[test]
fn rotates_at_5_mb() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fresh_temp_dir("log");
    log_init(&dir);
    let chunk = "x".repeat(1000);
    for _ in 0..30000 {
        log_msg!(LogLevel::Debug, "{}", chunk); // ~30 MB
    }
    log_shutdown();
    assert!(dir.join("airplay.1.log").exists());
    let files = std::fs::read_dir(&dir).unwrap().count();
    assert!(files <= 5);
    let size = std::fs::metadata(dir.join("airplay.log")).unwrap().len();
    assert!(size <= 5 * 1024 * 1024 + 2048);
}

#[test]
fn log_before_init_does_not_panic() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    log_shutdown();
    log_msg!(LogLevel::Warn, "ohne Datei {}", 1);
}

#[test]
fn raop_bridge_maps_levels_and_strips_newlines() {
    use std::ffi::CString;
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fresh_temp_dir("log");
    log_init(&dir);
    let m = CString::new("hallo\r\n").unwrap();
    unsafe { airplay::app::log::log_raop_bridge(std::ptr::null_mut(), 3, m.as_ptr()) };
    unsafe { airplay::app::log::log_raop_bridge(std::ptr::null_mut(), 7, m.as_ptr()) };
    unsafe { airplay::app::log::log_raop_bridge(std::ptr::null_mut(), 6, std::ptr::null()) };
    log_shutdown();
    let text = std::fs::read_to_string(dir.join("airplay.log")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].ends_with("[ERROR] [raop] hallo"), "{}", lines[0]);
    assert!(lines[1].ends_with("[DEBUG] [raop] hallo"), "{}", lines[1]);
    assert!(lines[2].ends_with("[INFO] [raop] "), "{}", lines[2]);
}
