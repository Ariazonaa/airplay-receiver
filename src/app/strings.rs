//! Oberflächentexte (Deutsch/Englisch). Platzhalter sind `{}`; `trf` ersetzt sie der Reihe nach.
//! Zahlen (z. B. `%.0f`, `%02d` in v1) formatiert der Aufrufer vor `trf`.

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

/// Alle Oberflächentexte; Reihenfolge = `TABLE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Str {
    WindowTitle,
    WaitingForConnection,
    FirewallBlocks,
    FirewallAddRule,
    FirewallConfirmPrompt,
    FirewallStillBlocked,
    FirewallDeclined,
    FirewallFailed,
    StatsLine1,
    StatsLine2,
    StatsLine3,
    HwGpu,
    HwSoftware,
    SoftwareDecodeHint,
    SettingsButton,
    SettingsTitle,
    ReceiverName,
    ModeSection,
    ModeLowLatency,
    ModeSmooth,
    SmoothDelay,
    NetworkSection,
    NetworkAdapter,
    AdapterAuto,
    AdapterVirtualSuffix,
    AllowH265,
    ShowStats,
    StorageSection,
    Screenshots,
    Recordings,
    Open,
    LanguageLabel,
    LanguageAuto,
    RestartWarning,
    Apply,
    Close,
    RecWaiting,
    RecTimer,
    ToastScreenshotSaved,
    ToastScreenshotFailed,
    ToastScreenshotBusy,
    ToastNoFrame,
    ToastSettingsApplied,
    ToastSettingsSaveFailed,
    ToastNoConnection,
    ToastPreviousRecordingBusy,
    ToastRecordingImpossible,
    ToastRecordFolderUnavailable,
    ToastRecordingStarted,
    ToastRecordingSaving,
    ToastRecordingSaved,
    ToastRecordingSavedParts,
    ToastRecordingFailed,
    ToastNothingRecorded,
    NoticeNoNetwork,
    NoticeAdapterUnavailable,
    NoticePortInUse,
    NoticeAnnounceFailed,
    NoticeStartFailed,
}

const ALL: [Str; 59] = [
    Str::WindowTitle,
    Str::WaitingForConnection,
    Str::FirewallBlocks,
    Str::FirewallAddRule,
    Str::FirewallConfirmPrompt,
    Str::FirewallStillBlocked,
    Str::FirewallDeclined,
    Str::FirewallFailed,
    Str::StatsLine1,
    Str::StatsLine2,
    Str::StatsLine3,
    Str::HwGpu,
    Str::HwSoftware,
    Str::SoftwareDecodeHint,
    Str::SettingsButton,
    Str::SettingsTitle,
    Str::ReceiverName,
    Str::ModeSection,
    Str::ModeLowLatency,
    Str::ModeSmooth,
    Str::SmoothDelay,
    Str::NetworkSection,
    Str::NetworkAdapter,
    Str::AdapterAuto,
    Str::AdapterVirtualSuffix,
    Str::AllowH265,
    Str::ShowStats,
    Str::StorageSection,
    Str::Screenshots,
    Str::Recordings,
    Str::Open,
    Str::LanguageLabel,
    Str::LanguageAuto,
    Str::RestartWarning,
    Str::Apply,
    Str::Close,
    Str::RecWaiting,
    Str::RecTimer,
    Str::ToastScreenshotSaved,
    Str::ToastScreenshotFailed,
    Str::ToastScreenshotBusy,
    Str::ToastNoFrame,
    Str::ToastSettingsApplied,
    Str::ToastSettingsSaveFailed,
    Str::ToastNoConnection,
    Str::ToastPreviousRecordingBusy,
    Str::ToastRecordingImpossible,
    Str::ToastRecordFolderUnavailable,
    Str::ToastRecordingStarted,
    Str::ToastRecordingSaving,
    Str::ToastRecordingSaved,
    Str::ToastRecordingSavedParts,
    Str::ToastRecordingFailed,
    Str::ToastNothingRecorded,
    Str::NoticeNoNetwork,
    Str::NoticeAdapterUnavailable,
    Str::NoticePortInUse,
    Str::NoticeAnnounceFailed,
    Str::NoticeStartFailed,
];

impl Str {
    pub const COUNT: usize = ALL.len();

    pub fn from_index(i: usize) -> Option<Str> {
        ALL.get(i).copied()
    }
}

/// (Deutsch, Englisch) je `Str`, gleiche Reihenfolge wie das enum.
const TABLE: [(&str, &str); Str::COUNT] = [
    ("AirPlay-Empfänger", "AirPlay Receiver"), // WindowTitle
    ("Warte auf Verbindung …", "Waiting for a connection …"), // WaitingForConnection
    (
        "Die Windows-Firewall lässt das iPhone noch nicht durch.",
        "Windows Firewall is still blocking the iPhone.",
    ), // FirewallBlocks
    ("Firewall-Regel anlegen", "Add firewall rule"), // FirewallAddRule
    (
        "Bitte die Windows-Abfrage bestätigen …",
        "Please confirm the Windows prompt …",
    ), // FirewallConfirmPrompt
    (
        "Regel angelegt, aber Windows blockiert weiter. Details im Log.",
        "Rule added, but Windows is still blocking. Details in the log.",
    ), // FirewallStillBlocked
    (
        "Abgelehnt. Ohne Regel kann das iPhone den PC nicht erreichen.",
        "Declined. Without the rule the iPhone cannot reach this PC.",
    ), // FirewallDeclined
    (
        "Das hat nicht geklappt. Details im Log.",
        "That did not work. Details in the log.",
    ), // FirewallFailed
    ("{} fps · {}x{} · {} · {}", "{} fps · {}x{} · {} · {}"), // StatsLine1
    (
        "Modus {} · Warteschlange {} · verworfen {}",
        "Mode {} · queue {} · dropped {}",
    ), // StatsLine2
    ("Audio-Puffer {} ms", "Audio buffer {} ms"), // StatsLine3
    ("GPU", "GPU"),                            // HwGpu
    ("Software", "software"),                  // HwSoftware
    (
        "Software-Dekodierung aktiv – höhere Verzögerung",
        "Software decoding active – higher latency",
    ), // SoftwareDecodeHint
    ("Einstellungen", "Settings"),             // SettingsButton
    ("Einstellungen", "Settings"),             // SettingsTitle
    ("Empfängername", "Receiver name"),        // ReceiverName
    ("Modus", "Mode"),                         // ModeSection
    ("Niedrige Latenz", "Low latency"),        // ModeLowLatency
    ("Flüssig", "Smooth"),                     // ModeSmooth
    ("Verzögerung (Flüssig)", "Delay (smooth)"), // SmoothDelay
    ("Netzwerk und Video", "Network and video"), // NetworkSection
    ("Netzwerkadapter", "Network adapter"),    // NetworkAdapter
    ("Automatisch", "Automatic"),              // AdapterAuto
    ("  (virtuell)", "  (virtual)"),             // AdapterVirtualSuffix
    ("H.265 erlauben", "Allow H.265"),         // AllowH265
    ("Statistik anzeigen", "Show statistics"), // ShowStats
    ("Speicherorte", "Storage"),               // StorageSection
    ("Screenshots", "Screenshots"),            // Screenshots
    ("Aufnahmen", "Recordings"),               // Recordings
    ("Öffnen", "Open"),                        // Open
    ("Sprache / Language", "Language / Sprache"), // LanguageLabel
    ("Automatisch (Windows)", "Automatic (Windows)"), // LanguageAuto
    (
        "Name, Netzwerkadapter oder H.265 ändern beendet eine laufende Verbindung.",
        "Changing the name, network adapter or H.265 ends an active connection.",
    ), // RestartWarning
    ("Übernehmen", "Apply"),                   // Apply
    ("Schließen", "Close"),                    // Close
    (
        "● REC – warte auf vollständiges Bild …",
        "● REC – waiting for a complete frame …",
    ), // RecWaiting
    ("● REC {}:{}", "● REC {}:{}"),            // RecTimer
    ("Screenshot gespeichert", "Screenshot saved"), // ToastScreenshotSaved
    (
        "Screenshot fehlgeschlagen – Details im Log",
        "Screenshot failed – details in the log",
    ), // ToastScreenshotFailed
    (
        "Screenshot wird noch gespeichert …",
        "Still saving the previous screenshot …",
    ), // ToastScreenshotBusy
    (
        "Kein Bild für einen Screenshot",
        "No picture to take a screenshot of",
    ), // ToastNoFrame
    ("Einstellungen übernommen", "Settings applied"), // ToastSettingsApplied
    (
        "Einstellungen konnten nicht gespeichert werden",
        "Settings could not be saved",
    ), // ToastSettingsSaveFailed
    (
        "Keine Verbindung – nichts aufzunehmen",
        "Not connected – nothing to record",
    ), // ToastNoConnection
    (
        "Vorherige Aufnahme wird noch gespeichert …",
        "Still saving the previous recording …",
    ), // ToastPreviousRecordingBusy
    (
        "Aufnahme nicht möglich – Details im Log",
        "Cannot record – details in the log",
    ), // ToastRecordingImpossible
    (
        "Aufnahmeordner nicht verfügbar: {}",
        "Recording folder not available: {}",
    ), // ToastRecordFolderUnavailable
    ("Aufnahme läuft", "Recording"),           // ToastRecordingStarted
    ("Aufnahme wird gespeichert …", "Saving recording …"), // ToastRecordingSaving
    ("Aufnahme gespeichert", "Recording saved"), // ToastRecordingSaved
    (
        "Aufnahme gespeichert ({} Teile)",
        "Recording saved ({} parts)",
    ), // ToastRecordingSavedParts
    (
        "Aufnahme fehlgeschlagen – Details im Log",
        "Recording failed – details in the log",
    ), // ToastRecordingFailed
    (
        "Nichts aufgenommen – es kam kein vollständiges Bild an",
        "Nothing recorded – no complete frame arrived",
    ), // ToastNothingRecorded
    (
        "Kein passendes Netzwerk gefunden. Ist der PC mit dem WLAN/LAN des iPhones verbunden?",
        "No suitable network found. Is this PC on the same Wi-Fi/LAN as the iPhone?",
    ), // NoticeNoNetwork
    (
        "Der gewählte Netzwerkadapter „{}“ ist nicht verfügbar.",
        "The selected network adapter \"{}\" is not available.",
    ), // NoticeAdapterUnavailable
    (
        "Port 7000 ist belegt – läuft der Empfänger oder ein anderes AirPlay-Programm schon?",
        "Port 7000 is in use – is the receiver or another AirPlay program already running?",
    ), // NoticePortInUse
    (
        "Der Empfänger konnte sich nicht im Netzwerk anmelden. Anderen Namen wählen oder Netzwerkadapter in den Einstellungen prüfen.",
        "The receiver could not announce itself on the network. Choose another name or check the network adapter in the settings.",
    ), // NoticeAnnounceFailed
    (
        "Der Empfänger konnte nicht starten. Details im Log.",
        "The receiver could not start. Details in the log.",
    ), // NoticeStartFailed
];

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn translate(s: Str, l: Lang) -> &'static str {
    let (de, en) = TABLE[s as usize];
    if l == Lang::En { en } else { de }
}

/// setting: "auto" | "de" | "en"; ui_lang_id: Windows-LANGID (GetUserDefaultUILanguage).
pub fn resolve_language(setting: &str, ui_lang_id: u16) -> Lang {
    match setting {
        "de" => Lang::De,
        "en" => Lang::En,
        // LANG_GERMAN = 0x07, primäre Sprache = untere 10 Bit
        _ if ui_lang_id & 0x3FF == 0x07 => Lang::De,
        _ => Lang::En,
    }
}

pub fn set_language(l: Lang) {
    LANG.store(l as u8, Ordering::SeqCst);
}

pub fn current_language() -> Lang {
    if LANG.load(Ordering::SeqCst) == Lang::En as u8 {
        Lang::En
    } else {
        Lang::De
    }
}

pub fn tr(s: Str) -> &'static str {
    translate(s, current_language())
}

/// Wie `tr`, ersetzt die `{}` der Reihe nach durch `args`. Fehlende Argumente
/// lassen `{}` stehen, überzählige werden ignoriert.
pub fn trf(s: Str, args: &[&dyn Display]) -> String {
    let text = tr(s);
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    let mut it = args.iter();
    while let Some(pos) = rest.find("{}") {
        out.push_str(&rest[..pos]);
        match it.next() {
            Some(a) => out.push_str(&a.to_string()),
            None => out.push_str("{}"),
        }
        rest = &rest[pos + 2..];
    }
    out.push_str(rest);
    out
}
