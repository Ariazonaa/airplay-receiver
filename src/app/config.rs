//! Einstellungen als JSON (Schlüssel wie v1), feldweiser Rückfall auf Standardwerte.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use windows::Win32::System::SystemInformation::{
    ComputerNamePhysicalDnsHostname, GetComputerNameExW,
};
use windows::core::PWSTR;

use crate::app::log::LogLevel;
use crate::log_msg;
use crate::platform::win::paths::{KnownDir, from_wide, known_dir};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    LowLatency,
    Smooth,
}

pub const SMOOTH_DELAY_MIN_MS: u32 = 100;
pub const SMOOTH_DELAY_MAX_MS: u32 = 500;
pub const SMOOTH_DELAY_DEFAULT_MS: u32 = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub receiver_name: String,
    pub mode: Mode,
    pub smooth_delay_ms: u32,
    pub allow_h265: bool,
    pub stats_overlay: bool,
    pub adapter: String,
    /// "auto" (Windows-Anzeigesprache) | "de" | "en"
    pub language: String,
    pub screenshot_dir: PathBuf,
    pub record_dir: PathBuf,
}

fn computer_name() -> String {
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    // SAFETY: buf und len passen zusammen; Windows schreibt höchstens len Zeichen.
    let ok = unsafe {
        GetComputerNameExW(
            ComputerNamePhysicalDnsHostname,
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
    };
    if ok.is_ok() && len > 0 {
        return from_wide(&buf[..len as usize]);
    }
    "AirPlay-PC".to_string()
}

fn known_or_empty(d: KnownDir) -> PathBuf {
    known_dir(d).unwrap_or_default()
}

/// Name = Computername, Ordner = Bilder\AirPlay bzw. Videos\AirPlay.
pub fn default_config() -> Config {
    Config {
        receiver_name: computer_name(),
        mode: Mode::LowLatency,
        smooth_delay_ms: SMOOTH_DELAY_DEFAULT_MS,
        allow_h265: false,
        stats_overlay: false,
        adapter: "auto".to_string(),
        language: "auto".to_string(),
        screenshot_dir: known_or_empty(KnownDir::Pictures).join("AirPlay"),
        record_dir: known_or_empty(KnownDir::Videos).join("AirPlay"),
    }
}

/// `%APPDATA%\airplay\config.json`
pub fn config_file() -> PathBuf {
    known_or_empty(KnownDir::AppData)
        .join("airplay")
        .join("config.json")
}

/// Liest ein Feld; bei falschem Typ Warnung im Log und `None`.
fn field<T>(j: &Value, key: &str, get: impl Fn(&Value) -> Option<T>) -> Option<T> {
    let v = j.get(key)?;
    let r = get(v);
    if r.is_none() {
        log_msg!(
            LogLevel::Warn,
            "config: Feld '{}' hat falschen Typ, Standardwert bleibt",
            key
        );
    }
    r
}

fn as_string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

fn as_int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// Fehlende/kaputte Datei -> `default_config()`; einzelne ungültige Felder -> Standardwert.
pub fn load_config(file: &Path) -> Config {
    let mut c = default_config();
    let Ok(bytes) = std::fs::read(file) else {
        return c;
    };
    let j: Value = match serde_json::from_slice(&bytes) {
        Ok(v @ Value::Object(_)) => v,
        _ => {
            log_msg!(
                LogLevel::Warn,
                "config: Datei unlesbar, nutze Standardwerte"
            );
            return c;
        }
    };

    if let Some(name) = field(&j, "receiverName", as_string)
        && !name.trim().is_empty()
    {
        c.receiver_name = name;
    }

    // Wie v1: fehlt der Schlüssel oder ist der Wert unbekannt, gilt LowLatency.
    c.mode = match field(&j, "mode", as_string).as_deref() {
        Some("smooth") => Mode::Smooth,
        _ => Mode::LowLatency,
    };

    let delay = field(&j, "smoothDelayMs", as_int).unwrap_or(c.smooth_delay_ms as i64);
    c.smooth_delay_ms = delay.clamp(SMOOTH_DELAY_MIN_MS as i64, SMOOTH_DELAY_MAX_MS as i64) as u32;

    if let Some(b) = field(&j, "allowH265", |v| v.as_bool()) {
        c.allow_h265 = b;
    }
    if let Some(b) = field(&j, "statsOverlay", |v| v.as_bool()) {
        c.stats_overlay = b;
    }
    if let Some(s) = field(&j, "adapter", as_string) {
        c.adapter = s;
    }
    if let Some(s) = field(&j, "language", as_string) {
        c.language = s;
    }
    if c.language != "de" && c.language != "en" {
        c.language = "auto".to_string();
    }
    if c.adapter.is_empty() {
        c.adapter = "auto".to_string();
    }

    if let Some(d) = field(&j, "screenshotDir", as_string)
        && !d.is_empty()
    {
        c.screenshot_dir = PathBuf::from(d);
    }
    if let Some(d) = field(&j, "recordDir", as_string)
        && !d.is_empty()
    {
        c.record_dir = PathBuf::from(d);
    }
    c
}

/// Legt den Ordner an und schreibt atomar (temp + rename).
pub fn save_config(c: &Config, file: &Path) -> bool {
    let j = json!({
        "receiverName": c.receiver_name,
        "mode": if c.mode == Mode::Smooth { "smooth" } else { "low_latency" },
        "smoothDelayMs": c.smooth_delay_ms,
        "allowH265": c.allow_h265,
        "statsOverlay": c.stats_overlay,
        "adapter": c.adapter,
        "language": c.language,
        "screenshotDir": c.screenshot_dir.to_string_lossy(),
        "recordDir": c.record_dir.to_string_lossy(),
    });
    let Ok(text) = serde_json::to_string_pretty(&j) else {
        return false;
    };
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    if std::fs::write(&tmp, text).is_err() {
        return false;
    }
    if let Err(e) = std::fs::rename(&tmp, file) {
        log_msg!(LogLevel::Error, "config: Speichern fehlgeschlagen ({})", e);
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}
