//! Laut gemeldeter Skip für Tests, die auf einem CI-Runner ohne GPU (`AIRPLAY_CI=1`, gesetzt in
//! `.github/workflows/ci.yml`) nicht laufen können: echtes D3D11-Hardwaregerät bzw. D3D11VA,
//! Port 7000 und ein Netzwerkadapter für den Empfänger.
//!
//! `AIRPLAY_REQUIRE_RECEIVER_TEST=1` (setzt `scripts/sandbox.ps1 test`) hat Vorrang: Dort sind
//! GPU und Adapter vorhanden, ein Skip ist dann ein Fehler.
//!
//! Die Meldung geht direkt auf stderr und nicht über `eprintln!`: libtest fängt `eprintln!`
//! erfolgreicher Tests ab, ein übersprungener (also bestandener) Test bliebe sonst stumm.

use std::io::Write;

pub const CI_VAR: &str = "AIRPLAY_CI";
pub const REQUIRE_VAR: &str = "AIRPLAY_REQUIRE_RECEIVER_TEST";

#[derive(Debug, PartialEq, Eq)]
pub enum CiSkip {
    Run,
    Skip,
    Fail,
}

/// Reine Entscheidung aus den Werten von `AIRPLAY_CI` und `AIRPLAY_REQUIRE_RECEIVER_TEST`.
pub fn ci_decision(ci: Option<&str>, require: Option<&str>) -> CiSkip {
    match (ci == Some("1"), require == Some("1")) {
        (false, _) => CiSkip::Run,
        (true, false) => CiSkip::Skip,
        (true, true) => CiSkip::Fail,
    }
}

/// `WARNUNG: <msg>` an der Ausgabe-Erfassung von libtest vorbei direkt auf stderr, als ein
/// einziger Schreibvorgang (sonst schieben sich z. B. FFmpeg-Meldungen paralleler Tests dazwischen).
pub fn report_skip(msg: &str) {
    let line = format!("WARNUNG: {msg}\n");
    let _ = std::io::stderr().write_all(line.as_bytes());
}

/// `true`: Test `test` überspringen (gemeldet), weil er `needs` braucht und `AIRPLAY_CI=1`
/// gesetzt ist. Mit `AIRPLAY_REQUIRE_RECEIVER_TEST=1` schlägt der Test stattdessen fehl.
pub fn skip_on_ci(test: &str, needs: &str) -> bool {
    let var = |name: &str| std::env::var(name).ok();
    match ci_decision(var(CI_VAR).as_deref(), var(REQUIRE_VAR).as_deref()) {
        CiSkip::Run => false,
        CiSkip::Skip => {
            report_skip(&format!(
                "{test} übersprungen ({CI_VAR}=1): braucht {needs}"
            ));
            true
        }
        CiSkip::Fail => panic!(
            "{REQUIRE_VAR}=1, {test} wäre aber übersprungen worden ({CI_VAR}=1, braucht {needs})"
        ),
    }
}
