//! Entscheidung des CI-Skips (`tests/support/ci.rs`): ohne `AIRPLAY_CI=1` laufen alle Tests,
//! mit überspringen die Hardware-Tests, und `AIRPLAY_REQUIRE_RECEIVER_TEST=1` macht aus dem
//! Skip einen Fehler. Geprüft wird die reine Entscheidung aus den Variablenwerten, nicht die
//! prozessweite Umgebung (parallel laufende Tests würden sich sonst gegenseitig stören).

#[allow(dead_code)] // hier nur die reine Entscheidung
#[path = "support/ci.rs"]
mod ci;

use ci::{CiSkip, ci_decision};

#[test]
fn without_ci_everything_runs() {
    assert_eq!(ci_decision(None, None), CiSkip::Run);
    assert_eq!(ci_decision(None, Some("1")), CiSkip::Run);
    assert_eq!(ci_decision(Some("0"), None), CiSkip::Run);
    assert_eq!(ci_decision(Some(""), Some("1")), CiSkip::Run);
}

#[test]
fn ci_skips_hardware_tests() {
    assert_eq!(ci_decision(Some("1"), None), CiSkip::Skip);
    assert_eq!(ci_decision(Some("1"), Some("0")), CiSkip::Skip);
}

#[test]
fn require_receiver_test_wins_over_ci() {
    assert_eq!(ci_decision(Some("1"), Some("1")), CiSkip::Fail);
}
