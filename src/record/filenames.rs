//! Dateinamen für Screenshots und Aufnahmen, Keyframe-Erkennung in Annex-B-Paketen.

use std::path::{Path, PathBuf};

use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::protocol::packets::VideoPacket;

/// Ortszeit (Felder wie `SYSTEMTIME`: Monat 1–12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

impl LocalTime {
    pub fn now() -> LocalTime {
        // SAFETY: keine Vorbedingungen.
        let t = unsafe { GetLocalTime() };
        LocalTime {
            year: t.wYear,
            month: t.wMonth,
            day: t.wDay,
            hour: t.wHour,
            minute: t.wMinute,
            second: t.wSecond,
        }
    }
}

/// "AirPlay_2026-11-19_14-03-22.png"; leeres `ext` → ohne Endung.
pub fn timestamp_name(prefix: &str, t: &LocalTime, ext: &str) -> String {
    let mut name = format!(
        "{prefix}_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    );
    if !ext.is_empty() {
        name.push('.');
        name.push_str(ext);
    }
    name
}

/// `dir/file`, bei Kollision "<stem>_2<ext>", "<stem>_3<ext>", …
pub fn unique_path(dir: &Path, file: &str) -> PathBuf {
    let first = dir.join(file);
    if !first.exists() {
        return first;
    }
    let stem = first.file_stem().unwrap_or_default().to_os_string();
    let ext = first.extension().map(|e| {
        let mut s = std::ffi::OsString::from(".");
        s.push(e);
        s
    });
    for n in 2.. {
        let mut name = stem.clone();
        name.push(format!("_{n}"));
        if let Some(e) = &ext {
            name.push(e);
        }
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("unendlicher Zähler")
}

/// Teil 1: "<stem>.<ext>", ab Teil 2: "<stem>_teil<n>.<ext>"
pub fn part_file_name(stem: &str, part: i32, ext: &str) -> String {
    if part > 1 {
        format!("{stem}_teil{part}.{ext}")
    } else {
        format!("{stem}.{ext}")
    }
}

/// Enthält das Annex-B-Paket ein IDR-Bild (H.264 Typ 5, H.265 Typ 16–21)?
pub fn contains_keyframe(p: &VideoPacket) -> bool {
    let d = &p.data;
    let mut i = 0;
    while i + 3 < d.len() {
        if d[i] != 0 || d[i + 1] != 0 {
            i += 1;
            continue;
        }
        let hdr = if d[i + 2] == 1 {
            i + 3
        } else if d[i + 2] == 0 && d[i + 3] == 1 {
            i + 4
        } else {
            i += 1;
            continue;
        };
        if hdr >= d.len() {
            break;
        }
        if p.h265 {
            let ty = (d[hdr] >> 1) & 0x3f;
            if (16..=21).contains(&ty) {
                return true;
            }
        } else if d[hdr] & 0x1f == 5 {
            return true;
        }
        // v1: i = hdr, danach ++i
        i = hdr + 1;
    }
    false
}
