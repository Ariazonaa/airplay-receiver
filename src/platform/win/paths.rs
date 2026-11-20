//! Bekannte Windows-Ordner und UTF-16-Umwandlung.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_Pictures, FOLDERID_RoamingAppData, FOLDERID_Videos,
    KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnownDir {
    AppData,
    LocalAppData,
    Pictures,
    Videos,
}

/// Pfad eines bekannten Ordners; `None`, wenn Windows ihn nicht liefert.
pub fn known_dir(d: KnownDir) -> Option<PathBuf> {
    let id = match d {
        KnownDir::AppData => &FOLDERID_RoamingAppData,
        KnownDir::LocalAppData => &FOLDERID_LocalAppData,
        KnownDir::Pictures => &FOLDERID_Pictures,
        KnownDir::Videos => &FOLDERID_Videos,
    };
    // SAFETY: SHGetKnownFolderPath liefert bei Erfolg eine nullterminierte, mit
    // CoTaskMemAlloc angelegte Zeichenkette, die wir kopieren und danach freigeben.
    unsafe {
        let raw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = OsString::from_wide(raw.as_wide());
        CoTaskMemFree(Some(raw.as_ptr() as *const _));
        if path.is_empty() {
            None
        } else {
            Some(PathBuf::from(path))
        }
    }
}

/// UTF-8 -> UTF-16, nullterminiert.
pub fn to_wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = std::ffi::OsStr::new(s).encode_wide().collect();
    v.push(0);
    v
}

/// UTF-16 (ohne oder mit Nullterminator) -> UTF-8; ungültige Teile werden ersetzt.
pub fn from_wide(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_roundtrip_with_umlauts_and_emoji() {
        let s = "Küchen-PC 📺";
        let w = to_wide(s);
        assert_eq!(*w.last().unwrap(), 0);
        assert_eq!(from_wide(&w), s);
        assert_eq!(from_wide(&w[..w.len() - 1]), s);
    }

    #[test]
    fn known_dirs_exist() {
        for d in [
            KnownDir::AppData,
            KnownDir::LocalAppData,
            KnownDir::Pictures,
            KnownDir::Videos,
        ] {
            let p = known_dir(d).unwrap();
            assert!(p.is_absolute(), "{d:?}: {p:?}");
        }
    }
}
