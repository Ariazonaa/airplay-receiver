//! airplay-probe.exe: Diagnose ohne Fenster. Startet den Empfänger (Ankündigung +
//! AirPlay-Protokoll), zeigt Ereignisse und Paketzähler. Beenden mit Strg+C.
//!   airplay-probe.exe [--name NAME] [--adapter NAME] [--h265] [--verbose] [--no-firewall]
//!   airplay-probe.exe --firewall <exe-pfad>   legt die Firewall-Regel für dieses Programm an (UAC)

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::System::Console::{SetConsoleCtrlHandler, SetConsoleOutputCP};
use windows::core::BOOL;

use airplay::app::config::default_config;
use airplay::app::log::{LogLevel, log_init, log_shutdown};
use airplay::discovery::adapters::{enumerate_adapters, looks_virtual, pick_adapter};
use airplay::log_msg;
use airplay::platform::win::firewall::{
    FirewallResult, RULE_NAME, add_firewall_rule_elevated, firewall_ready,
};
use airplay::platform::win::paths::{KnownDir, known_dir};
use airplay::protocol::packets::SessionEvent;
use airplay::protocol::receiver::{Receiver, ReceiverOptions};

const CP_UTF8: u32 = 65001;

static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn on_ctrl(_ctrl_type: u32) -> BOOL {
    STOP.store(true, Ordering::SeqCst);
    BOOL(1)
}

fn say(text: &str) {
    println!("{text}");
    log_msg!(LogLevel::Info, "probe: {}", text);
}

fn firewall_text(r: FirewallResult) -> &'static str {
    match r {
        FirewallResult::Added => "Regel angelegt",
        FirewallResult::Declined => "abgelehnt – das iPhone kann den PC evtl. nicht erreichen",
        FirewallResult::Failed => "Fehler beim Anlegen",
    }
}

fn exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    // SAFETY: keine Vorbedingungen; on_ctrl setzt nur ein Atomic.
    unsafe {
        let _ = SetConsoleOutputCP(CP_UTF8);
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
    }
    if let Some(dir) = known_dir(KnownDir::LocalAppData) {
        log_init(&dir.join("airplay").join("logs"));
    }

    let mut cfg = default_config();
    let mut verbose = false;
    let mut check_firewall = true;
    let mut firewall_for: Option<String> = None;
    let mut args = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned());
    while let Some(a) = args.next() {
        match a.as_str() {
            "--name" => {
                if let Some(v) = args.next() {
                    cfg.receiver_name = v;
                }
            }
            "--adapter" => {
                if let Some(v) = args.next() {
                    cfg.adapter = v;
                }
            }
            "--h265" => cfg.allow_h265 = true,
            "--verbose" => verbose = true,
            "--no-firewall" => check_firewall = false,
            "--firewall" => firewall_for = Some(args.next().unwrap_or_default()),
            _ => {}
        }
    }

    if let Some(exe) = firewall_for {
        if exe.is_empty() {
            say("Aufruf: airplay-probe --firewall <exe-pfad>");
            log_shutdown();
            return 2;
        }
        say(&format!(
            "Firewall-Regel \"{RULE_NAME}\" für {exe}, frage Admin-Rechte an …"
        ));
        let r = add_firewall_rule_elevated(&exe, RULE_NAME);
        say(&format!("Firewall: {}", firewall_text(r)));
        log_shutdown();
        return if r == FirewallResult::Added { 0 } else { 1 };
    }

    let rule_name = "AirPlay-Empfänger (Probe)";
    if check_firewall && !firewall_ready(&exe_path(), rule_name) {
        say("Firewall-Regel fehlt, frage Admin-Rechte an …");
        let r = add_firewall_rule_elevated(&exe_path(), rule_name);
        say(&format!("Firewall: {}", firewall_text(r)));
    }

    let adapters = enumerate_adapters();
    for a in &adapters {
        log_msg!(
            LogLevel::Info,
            "adapter [{}] {} ({}) typ={} up={} ip={} virtuell={}",
            a.if_index,
            a.name,
            a.description,
            a.if_type,
            u8::from(a.up),
            a.ipv4.first().map_or("-", String::as_str),
            u8::from(looks_virtual(a))
        );
    }
    let Some(chosen) = pick_adapter(&adapters, &cfg.adapter) else {
        say(&format!(
            "Kein passender Netzwerkadapter für '{}'.",
            cfg.adapter
        ));
        log_shutdown();
        return 1;
    };
    say(&format!(
        "Adapter: {}, {}",
        chosen.name,
        chosen.ipv4.first().map_or("?", String::as_str)
    ));

    let mut receiver = Receiver::new();
    let opts = ReceiverOptions {
        name: cfg.receiver_name.clone(),
        adapter: chosen,
        allow_h265: cfg.allow_h265,
        verbose_log: verbose,
    };
    if !receiver.start(&opts) {
        say("Empfänger startet nicht – Details im Log.");
        log_shutdown();
        return 1;
    }
    say(&format!(
        "Empfänger \"{}\" läuft auf Port {}. Auf dem iPhone Bildschirmsynchronisierung wählen. Strg+C beendet.",
        cfg.receiver_name,
        receiver.port()
    ));

    let (mut video_count, mut audio_count, mut video_bytes) = (0u64, 0u64, 0u64);
    let mut dumped_first = false;
    let mut last_report = Instant::now();
    while !STOP.load(Ordering::SeqCst) {
        receiver.poll();
        for e in receiver.events().drain() {
            match e {
                SessionEvent::VideoFormat {
                    width,
                    height,
                    h265,
                } => say(&format!(
                    "Ereignis VideoFormat: {width}x{height} {}",
                    if h265 { "H.265" } else { "" }
                )),
                SessionEvent::Volume { db } => say(&format!("Ereignis Volume: {db:.1} dB")),
                SessionEvent::Connected { device } => say(&format!("Ereignis Connected {device}")),
                SessionEvent::Disconnected => say("Ereignis Disconnected "),
                SessionEvent::Reset => say("Ereignis Reset "),
            }
        }
        for v in receiver.video().drain() {
            if !dumped_first && !v.data.is_empty() {
                let mut hex = String::new();
                for b in v.data.iter().take(32) {
                    let _ = write!(hex, "{b:02x} ");
                }
                say(&format!(
                    "Erstes Videopaket ({} Bytes, {}): {hex}",
                    v.data.len(),
                    if v.h265 { "H.265" } else { "H.264" }
                ));
                dumped_first = true;
            }
            video_count += 1;
            video_bytes += v.data.len() as u64;
        }
        let audio = receiver.audio().drain();
        let last_ct = audio.last().map_or(0, |a| a.ct);
        audio_count += audio.len() as u64;
        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            if receiver.open_connections() > 0 || video_count > 0 || audio_count > 0 {
                say(&format!(
                    "Video {video_count} Pakete ({} kB), Audio {audio_count} Pakete (ct={last_ct}), Verbindungen {}, verworfen v={} a={}",
                    video_bytes / 1024,
                    receiver.open_connections(),
                    receiver.video().dropped(),
                    receiver.audio().dropped()
                ));
            }
            video_count = 0;
            audio_count = 0;
            video_bytes = 0;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    receiver.stop();
    log_shutdown();
    0
}
