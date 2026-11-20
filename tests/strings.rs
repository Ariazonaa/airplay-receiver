//! Port von test_strings.cpp.
use std::sync::Mutex;

use airplay::app::strings::{self, *};

/// Die aktuelle Sprache ist global; Tests, die sie lesen oder setzen, laufen nacheinander.
static LANG: Mutex<()> = Mutex::new(());

/// Folge der Platzhalter-Konvertierungen: hier nur die Anzahl der `{}`.
fn placeholders(s: &str) -> usize {
    s.matches("{}").count()
}

#[test]
fn every_key_has_text_in_both_languages() {
    for i in 0..Str::COUNT {
        let s = Str::from_index(i).unwrap();
        assert!(!translate(s, Lang::De).is_empty(), "{i}");
        assert!(!translate(s, Lang::En).is_empty(), "{i}");
    }
}

#[test]
fn placeholders_match_between_languages() {
    for i in 0..Str::COUNT {
        let s = Str::from_index(i).unwrap();
        let (de, en) = (translate(s, Lang::De), translate(s, Lang::En));
        assert_eq!(placeholders(de), placeholders(en), "{i}: {de} / {en}");
        assert!(!de.contains('%') && !en.contains('%'), "{i}: printf-Rest");
    }
}

#[test]
fn resolve_language() {
    assert_eq!(strings::resolve_language("de", 0x0409), Lang::De);
    assert_eq!(strings::resolve_language("en", 0x0407), Lang::En);
    assert_eq!(strings::resolve_language("auto", 0x0407), Lang::De); // de-DE
    assert_eq!(strings::resolve_language("auto", 0x0C07), Lang::De); // de-AT
    assert_eq!(strings::resolve_language("auto", 0x0807), Lang::De); // de-CH
    assert_eq!(strings::resolve_language("auto", 0x0409), Lang::En); // en-US
    assert_eq!(strings::resolve_language("auto", 0x040C), Lang::En); // fr-FR -> Englisch
    assert_eq!(strings::resolve_language("xx", 0x0409), Lang::En);
}

#[test]
fn tr_follows_setlanguage() {
    let _g = LANG.lock().unwrap_or_else(|e| e.into_inner());
    set_language(Lang::En);
    assert_eq!(tr(Str::WindowTitle), "AirPlay Receiver");
    set_language(Lang::De);
    assert_eq!(tr(Str::WindowTitle), "AirPlay-Empfänger");
    assert_eq!(current_language(), Lang::De);
}

#[test]
fn trf_fills_placeholders_in_order() {
    let _g = LANG.lock().unwrap_or_else(|e| e.into_inner());
    set_language(Lang::De);
    let t = trf(Str::NoticeAdapterUnavailable, &[&"WLAN"]);
    assert!(t.contains("„WLAN“"), "{t}");
    let t = trf(Str::StatsLine1, &[&60, &1920, &1080, &"H.264", &"GPU"]);
    assert_eq!(t, "60 fps · 1920x1080 · H.264 · GPU");
}

#[test]
fn table_matches_v1_whitespace() {
    for i in 0..Str::COUNT {
        let s = Str::from_index(i).unwrap();
        let (de, en) = (translate(s, Lang::De), translate(s, Lang::En));
        let lead = |t: &str| t.len() - t.trim_start().len();
        let trail = |t: &str| t.len() - t.trim_end().len();
        assert_eq!(
            lead(de),
            lead(en),
            "{i}: führende Leerzeichen {de:?} / {en:?}"
        );
        assert_eq!(
            trail(de),
            trail(en),
            "{i}: nachgestellte Leerzeichen {de:?} / {en:?}"
        );
    }
}
