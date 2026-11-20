//! Beide Exes müssen ein eingebettetes Icon (Ressource-ID 1) tragen.

use windows::Win32::UI::Shell::ExtractIconExW;
use windows::core::HSTRING;

/// Anzahl der Icons in der Exe (`ExtractIconExW` mit Index -1).
fn icon_count(exe: &str) -> u32 {
    // SAFETY: gültiger, nullterminierter Pfad; keine Ausgabe-Handles angefordert.
    unsafe { ExtractIconExW(&HSTRING::from(exe), -1, None, None, 0) }
}

#[test]
fn exe_has_icon_resource() {
    assert!(
        icon_count(env!("CARGO_BIN_EXE_airplay")) >= 1,
        "airplay.exe ohne Icon"
    );
    assert!(
        icon_count(env!("CARGO_BIN_EXE_airplay-probe")) >= 1,
        "airplay-probe.exe ohne Icon"
    );
}
