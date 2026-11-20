//! Die Anwendung wie v1 `app.cpp`: Fenster, Renderer, Empfänger, Sitzungssteuerung, Bild- und
//! Tonpfad, Tastenkürzel, Einstellungen übernehmen, Netzwerküberwachung und Hintergrundaufgaben.
//!
//! Bild: Dekodieren im Haupt-Thread → Bildring → `FrameScheduler`; Ton: eigener Thread
//! `AudioPipeline` → SDL-Ausgabe. "Flüssig" nutzt für das Bild denselben Versatz wie der
//! Ton-Thread.
//!
//! Windows hält beim Verschieben/Größe-Ändern den Fenster-Thread in einer eigenen Schleife
//! fest. SDL schickt dann EXPOSED-Ereignisse mit `data1 == 1`, in denen direkt gezeichnet
//! werden darf – so läuft das Bild weiter (Event-Watch `live_redraw`).
//!
//! Die Oberfläche (egui, `ui.rs`) liest `UiState` und meldet Wünsche über `UiState::actions`;
//! sie wird nach dem Videobild und vor Present gezeichnet (auch in `live_redraw`).

use std::cell::{Cell, UnsafeCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{JoinHandle, ThreadId};

use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::core::PCWSTR;

use crate::app::config::{Config, Mode, config_file, default_config, load_config, save_config};
use crate::app::demo::{DemoSource, Options};
use crate::app::egui_input::{EguiInput, egui_wants_text};
use crate::app::hotkeys::{Command, hotkey_command};
use crate::app::log::{LogLevel, log_init, log_shutdown};
use crate::app::session_controller::{SessionAction, SessionController, SessionState};
use crate::app::settings_apply::{ApplyEffect, diff_config};
use crate::app::strings::{Lang, Str, resolve_language, set_language, tr, trf};
use crate::app::ui::{UiAction, UiState, draw_ui, init_ui_fonts};
use crate::app::window::{Window, poll_event, ticks};
use crate::discovery::adapters::{
    adapter_changed, enumerate_adapters, looks_virtual, pick_adapter,
};
use crate::ffi::sdl3::events::{
    SDL_AddEventWatch, SDL_EVENT_KEY_DOWN, SDL_EVENT_MOUSE_MOTION, SDL_EVENT_QUIT,
    SDL_EVENT_WINDOW_CLOSE_REQUESTED, SDL_EVENT_WINDOW_ENTER_FULLSCREEN, SDL_EVENT_WINDOW_EXPOSED,
    SDL_EVENT_WINDOW_LEAVE_FULLSCREEN, SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED, SDL_Event,
    SDL_RemoveEventWatch,
};
use crate::ffi::sdl3::timer::SDL_Delay;
use crate::log_msg;
use crate::media::audio_decoder::volume_to_gain;
use crate::media::audio_output::AudioOutput;
use crate::media::audio_pipeline::AudioPipeline;
use crate::media::scheduler::{FrameScheduler, LeadEstimator};
use crate::media::video_decoder::{Frame, VideoDecoder};
use crate::platform::win::firewall::{
    FirewallResult, RULE_NAME, add_firewall_rule_elevated, firewall_ready,
};
use crate::platform::win::paths::{KnownDir, known_dir, to_wide};
use crate::protocol::packets::SessionEvent;
use crate::protocol::receiver::{Receiver, ReceiverError, ReceiverOptions, local_time};
use crate::record::filenames::{LocalTime, timestamp_name, unique_path};
use crate::record::recorder::{Recorder, VideoFormat};
use crate::record::screenshot::save_screenshot;
use crate::render::egui_painter::EguiPainter;
use crate::render::letterbox::letterbox;
use crate::render::renderer::Renderer;

/// 500 ms "Flüssig" bei 60 fps = 30 Bilder + Reserve; passt zu `EXTRA_HW_FRAMES` im Decoder.
pub const MAX_PENDING_FRAMES: usize = 40;
/// Netzwerk prüfen (Adapter-/IP-Wechsel, Netz wieder da) alle 5 s.
const NET_CHECK_INTERVAL_MS: u64 = 5000;
/// Kurze Einblendung
const TOAST_MS: u64 = 2500;
/// Aufnahme-Status (Keyframe fehlt, Fehler) abfragen alle 1 s.
const RECORD_CHECK_INTERVAL_MS: u64 = 1000;
/// winuser.h
const SW_SHOWNORMAL: i32 = 1;

/// Hintergrundaufgabe mit Ergebnis über `mpsc` (v1: `std::future`).
struct Background<T> {
    rx: mpsc::Receiver<T>,
    thread: JoinHandle<()>,
}

impl<T: Send + 'static> Background<T> {
    fn spawn(name: &str, job: impl FnOnce() -> T + Send + 'static) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let _ = tx.send(job());
            });
        match spawned {
            Ok(thread) => Some(Self { rx, thread }),
            Err(e) => {
                log_msg!(
                    LogLevel::Error,
                    "app: Hintergrund-Thread {} startet nicht: {}",
                    name,
                    e
                );
                None
            }
        }
    }

    /// Wartet, bis der Thread fertig ist (Ergebnis wird verworfen).
    fn wait(self) {
        let _ = self.thread.join();
    }
}

/// Fertiges Ergebnis abholen und die Aufgabe entfernen. `Some(None)`: Thread ohne Ergebnis
/// beendet (Panik); `None`: läuft noch oder keine Aufgabe.
fn finished<T>(slot: &mut Option<Background<T>>) -> Option<Option<T>> {
    let r = match slot.as_ref()?.rx.try_recv() {
        Ok(v) => Some(v),
        Err(mpsc::TryRecvError::Empty) => return None,
        Err(mpsc::TryRecvError::Disconnected) => None,
    };
    *slot = None;
    Some(r)
}

/// Feinere Sleep-Auflösung für den Ton-Thread, solange die App lebt.
struct TimerPeriod;

impl TimerPeriod {
    fn begin() -> Self {
        // SAFETY: keine Vorbedingungen; Gegenstück in Drop.
        unsafe { timeBeginPeriod(1) };
        Self
    }
}

impl Drop for TimerPeriod {
    fn drop(&mut self) {
        // SAFETY: passend zu timeBeginPeriod in `begin`.
        unsafe { timeEndPeriod(1) };
    }
}

fn exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn apply_language(config: &Config) {
    // SAFETY: keine Vorbedingungen.
    let lang = resolve_language(&config.language, unsafe { GetUserDefaultUILanguage() });
    set_language(lang);
    log_msg!(
        LogLevel::Info,
        "app: Sprache {} (Einstellung {})",
        if lang == Lang::En {
            "Englisch"
        } else {
            "Deutsch"
        },
        config.language
    );
}

fn on_off(on: bool) -> &'static str {
    if on { "an" } else { "aus" }
}

/// egui-Kontext mit Segoe UI und dem dunklen Stil (v1 `initUiFonts`).
fn new_egui_context() -> egui::Context {
    let ctx = egui::Context::default();
    init_ui_fonts(&ctx);
    ctx
}

/// Ordner anlegen und im Explorer öffnen.
fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let verb = to_wide("open");
    let file: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    // SAFETY: alle Zeichenketten sind nullterminiert und leben bis nach dem Aufruf.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        log_msg!(
            LogLevel::Warn,
            "app: Ordner {} lässt sich nicht öffnen: {}",
            dir.display(),
            e
        );
    }
}

/// Alles, was die App besitzt (v1 `App::Impl`). Felder werden in Deklarationsreihenfolge
/// freigegeben: Fenster (SDL_Quit) und Timer zuletzt.
struct Inner {
    config: Config,
    ui: UiState,
    session: SessionController,
    receiver: Receiver,
    /// Demo-Modus (`--demo`): liefert die Videopakete statt des Empfängers
    demo: Option<DemoSource>,
    /// eigener Thread, unabhängig vom Fenster
    audio: AudioPipeline,
    audio_out: Arc<AudioOutput>,
    decoder: VideoDecoder,
    video_sched: FrameScheduler,
    /// Vorlauf des iPhones beim Bild (Ersatz, solange kein Ton läuft)
    video_lead_est: LeadEstimator,
    /// dekodiert, wartet auf Anzeige (Bild-ID, Bild)
    frame_ring: VecDeque<(u64, Frame)>,
    next_frame_id: u64,
    /// beim Überlauf des Bildrings verworfen
    ring_dropped: u64,
    /// für Screenshots
    last_shown: Option<Frame>,
    /// Maße des zuletzt dekodierten Bildes (Formatwechsel → neuer Aufnahme-Teil)
    last_decoded: (i32, i32),
    /// bekommt jedes Videopaket; der Ton-Thread und der Abschluss-Thread teilen ihn
    recorder: Arc<Recorder>,
    /// liest der Ton-Thread (PCM-Abgriff nur während der Aufnahme)
    recording_audio: Arc<AtomicBool>,
    /// Abschließen der Aufnahme im Hintergrund; liefert die Meldung
    recorder_stopping: Option<Background<String>>,
    /// PNG-Kodierung im Hintergrund
    screenshot_pending: Option<Background<bool>>,
    last_record_check_ticks: u64,
    record_start_ticks: u64,
    /// erste Firewall-Prüfung beim Start
    firewall_check: Option<Background<bool>>,
    /// UAC-Abfrage läuft im Hintergrund: (Ergebnis, danach bereit?)
    firewall_pending: Option<Background<(FirewallResult, bool)>>,
    last_net_check_ticks: u64,
    frames_this_second: u64,
    last_stats_tick: u64,
    fullscreen: bool,
    on_top: bool,
    running: bool,
    shut_down: bool,
    egui_ctx: egui::Context,
    egui_input: EguiInput,
    /// fehlt, wenn die egui-Objekte nicht angelegt werden konnten (dann nur Video)
    painter: Option<EguiPainter>,
    renderer: Option<Renderer>,
    window: Window,
    _timer: TimerPeriod,
}

impl Inner {
    fn init(config: Config, demo: Option<DemoSource>) -> Result<Inner, String> {
        let ui = UiState::new(&config);
        apply_language(&config);

        let timer = TimerPeriod::begin();
        let window = Window::create(tr(Str::WindowTitle))?;
        let renderer = Renderer::init(window.raw())?;
        let painter = EguiPainter::new(renderer.device()).ok();

        // Im Demo-Modus öffnet die App keine Ports – keine Firewall-Prüfung.
        let firewall_check = if demo.is_none() {
            let exe = exe_path();
            Background::spawn("firewall", move || firewall_ready(&exe, RULE_NAME))
        } else {
            None
        };

        let receiver = Receiver::new();
        let audio_out = Arc::new(AudioOutput::default());
        let audio = AudioPipeline::new(
            receiver.audio().clone(),
            audio_out.clone(),
            Box::new(local_time),
        );
        let recorder = Arc::new(Recorder::default());
        let recording_audio = Arc::new(AtomicBool::new(false));
        {
            let recorder = recorder.clone();
            let recording_audio = recording_audio.clone();
            audio.set_pcm_tap(Some(Box::new(move |pcm: &[f32], ts: u64| {
                if recording_audio.load(Ordering::SeqCst) {
                    recorder.push_audio(pcm, ts);
                }
            })));
        }
        let mut s = Inner {
            config,
            ui,
            session: SessionController::default(),
            receiver,
            demo,
            audio,
            audio_out,
            decoder: VideoDecoder::default(),
            video_sched: FrameScheduler::default(),
            video_lead_est: LeadEstimator::default(),
            frame_ring: VecDeque::with_capacity(MAX_PENDING_FRAMES + 1),
            next_frame_id: 0,
            ring_dropped: 0,
            last_shown: None,
            last_decoded: (0, 0),
            recorder,
            recording_audio,
            recorder_stopping: None,
            screenshot_pending: None,
            last_record_check_ticks: 0,
            record_start_ticks: 0,
            firewall_check,
            firewall_pending: None,
            last_net_check_ticks: 0,
            frames_this_second: 0,
            last_stats_tick: 0,
            fullscreen: false,
            on_top: false,
            running: true,
            shut_down: false,
            egui_ctx: new_egui_context(),
            egui_input: EguiInput::new(),
            painter,
            renderer: Some(renderer),
            window,
            _timer: timer,
        };
        s.apply_mode();
        s.audio.start();
        if s.demo.is_some() {
            s.start_demo();
        } else {
            s.start_receiver();
        }
        s.update_title();
        Ok(s)
    }

    fn tick(&mut self) {
        self.handle_session();
        if !self.window.minimized() {
            self.frame();
        }
    }

    fn resize(&mut self, w: i32, h: i32) {
        if let Some(r) = self.renderer.as_mut() {
            r.resize(w, h);
        }
    }

    fn handle_event(&mut self, e: &SDL_Event) {
        // v1: ImGui_ImplSDL3_ProcessEvent zuerst
        let for_egui = self.egui_input.on_event(e);
        // SAFETY: Der Typ steht in jedem Ereignis; die Unterstrukturen nur beim passenden Typ.
        let ty = unsafe { e.r#type };
        if ty == SDL_EVENT_QUIT.0 {
            log_msg!(LogLevel::Info, "app: Beenden angefordert (SDL_QUIT)");
            self.running = false;
        } else if ty == SDL_EVENT_WINDOW_CLOSE_REQUESTED.0 {
            // SAFETY: Fensterereignis.
            if unsafe { e.window.windowID } == self.window.id() {
                log_msg!(LogLevel::Info, "app: Fenster geschlossen");
                self.running = false;
            }
        } else if ty == SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED.0 {
            // SAFETY: Fensterereignis.
            let (w, h) = unsafe { (e.window.data1, e.window.data2) };
            self.resize(w, h);
        } else if ty == SDL_EVENT_MOUSE_MOTION.0 {
            self.ui.last_mouse_move_ticks = ticks();
        } else if ty == SDL_EVENT_WINDOW_ENTER_FULLSCREEN.0 {
            self.fullscreen = true;
        } else if ty == SDL_EVENT_WINDOW_LEAVE_FULLSCREEN.0 {
            self.fullscreen = false;
        } else if ty == SDL_EVENT_KEY_DOWN.0 {
            // SAFETY: Tastaturereignis.
            let k = unsafe { e.key };
            if !k.repeat
                && !for_egui
                && let Some(c) =
                    hotkey_command(k.key, k.r#mod, self.fullscreen, self.ui.wants_keyboard)
            {
                self.handle_command(c);
            }
        }
    }

    fn handle_command(&mut self, c: Command) {
        match c {
            Command::ToggleFullscreen => {
                self.fullscreen = !self.fullscreen;
                self.window.set_fullscreen(self.fullscreen);
                log_msg!(LogLevel::Info, "app: Vollbild {}", on_off(self.fullscreen));
            }
            Command::ExitFullscreen => {
                self.fullscreen = false;
                self.window.set_fullscreen(false);
                log_msg!(LogLevel::Info, "app: Vollbild aus");
            }
            Command::ToggleOnTop => {
                self.on_top = !self.on_top;
                self.window.set_always_on_top(self.on_top);
                log_msg!(
                    LogLevel::Info,
                    "app: Immer im Vordergrund {}",
                    on_off(self.on_top)
                );
            }
            Command::Screenshot => self.take_screenshot(),
            Command::ToggleRecording => self.toggle_recording(),
        }
    }

    fn handle_ui_action(&mut self, a: UiAction) {
        match a {
            UiAction::AddFirewallRule => self.add_firewall_rule(),
            UiAction::OpenSettings => self.open_settings(),
            UiAction::ApplySettings(next) => self.apply_settings(next),
            UiAction::OpenFolder(dir) => open_folder(&dir),
        }
    }

    fn add_firewall_rule(&mut self) {
        if self.firewall_pending.is_some() {
            return; // Abfrage läuft schon
        }
        self.ui.firewall_note = Some(Str::FirewallConfirmPrompt);
        let exe = exe_path();
        self.firewall_pending = Background::spawn("firewall", move || {
            let r = add_firewall_rule_elevated(&exe, RULE_NAME);
            let ready = r == FirewallResult::Added && firewall_ready(&exe, RULE_NAME);
            (r, ready)
        });
    }

    /// Arbeitskopie und Adapterliste füllen, erst dann das Fenster öffnen – so zeigt schon das
    /// erste Bild des Fensters die aktuellen Werte.
    fn open_settings(&mut self) {
        self.ui.draft = self.config.clone();
        self.ui.adapters = enumerate_adapters()
            .iter()
            .filter(|a| a.up && !a.ipv4.is_empty())
            .map(|a| (a.name.clone(), looks_virtual(a)))
            .collect();
        self.ui.settings_open = true;
    }

    fn handle_session(&mut self) {
        self.receiver.poll();
        for ev in self.receiver.events().drain() {
            if let SessionEvent::Volume { db } = ev {
                self.audio.set_gain(volume_to_gain(db));
            }
            let actions = self.session.on(&ev, self.ui.recording);
            self.apply_session_actions(&actions);
        }
        if let Some(demo) = self.demo.as_mut() {
            for p in demo.due(self.receiver.now_ns()) {
                self.receiver.video().push(p);
            }
        }
        for a in std::mem::take(&mut self.ui.actions) {
            self.handle_ui_action(a);
        }
        self.poll_background();
        self.check_network();
        self.ui.session = self.session.state();
        if self.ui.device != self.session.device() {
            self.ui.device = self.session.device().to_string();
        }
        self.decode_video();
        self.present_video();
        self.update_stats();
    }

    fn poll_background(&mut self) {
        if let Some(r) = finished(&mut self.firewall_check) {
            // Ohne Ergebnis (Panik) wie v1 bei einem Fehler: als fehlend anzeigen.
            self.ui.firewall_missing = !r.unwrap_or(false);
        }
        if let Some(r) = finished(&mut self.firewall_pending) {
            let (result, ready) = r.unwrap_or((FirewallResult::Failed, false));
            self.ui.firewall_note = match result {
                FirewallResult::Added => {
                    self.ui.firewall_missing = !ready;
                    (!ready).then_some(Str::FirewallStillBlocked)
                }
                FirewallResult::Declined => Some(Str::FirewallDeclined),
                FirewallResult::Failed => Some(Str::FirewallFailed),
            };
            log_msg!(LogLevel::Info, "firewall: Ergebnis {}", result as i32);
        }
        if let Some(r) = finished(&mut self.screenshot_pending) {
            self.show_toast(tr(if r.unwrap_or(false) {
                Str::ToastScreenshotSaved
            } else {
                Str::ToastScreenshotFailed
            }));
        }
        if let Some(r) = finished(&mut self.recorder_stopping) {
            let text = r.unwrap_or_else(|| tr(Str::ToastRecordingFailed).to_string());
            self.show_toast(&text);
        }
        let now = ticks();
        if self.ui.recording
            && now.saturating_sub(self.last_record_check_ticks) >= RECORD_CHECK_INTERVAL_MS
        {
            self.last_record_check_ticks = now;
            self.ui.record_waiting = self.recorder.waiting_for_keyframe();
            let err = self.recorder.last_error();
            if !err.is_empty() {
                log_msg!(LogLevel::Error, "app: Aufnahme-Fehler: {}", err);
                self.stop_recording();
            }
        }
    }

    fn apply_session_actions(&mut self, actions: &[SessionAction]) {
        for a in actions {
            match a {
                SessionAction::ShowIdle | SessionAction::ShowMirroring => self.update_title(),
                SessionAction::ResetMedia => self.reset_media(),
                SessionAction::StopRecording => self.stop_recording(),
            }
        }
    }

    fn restart_receiver(&mut self, why: &str) {
        if self.demo.is_some() {
            log_msg!(
                LogLevel::Info,
                "app: Demo-Modus, Empfänger bleibt aus ({})",
                why
            );
            return;
        }
        log_msg!(
            LogLevel::Info,
            "app: Empfänger wird neu gestartet ({})",
            why
        );
        // Laufende Sitzung endet: wie ein Verbindungsabbruch behandeln (inkl. Aufnahme beenden).
        let actions = self
            .session
            .on(&SessionEvent::Disconnected, self.ui.recording);
        self.apply_session_actions(&actions);
        self.reset_media();
        self.start_receiver();
    }

    fn check_network(&mut self) {
        if self.demo.is_some() {
            return; // kein Empfänger, nichts neu zu starten
        }
        let now = ticks();
        if now.saturating_sub(self.last_net_check_ticks) < NET_CHECK_INTERVAL_MS {
            return;
        }
        self.last_net_check_ticks = now;
        let adapter = pick_adapter(&enumerate_adapters(), &self.config.adapter);
        if !self.receiver.running() {
            // Beim Start war kein Netz da oder die Anmeldung scheiterte: erneut versuchen,
            // sobald ein Netz da ist.
            let e = self.receiver.last_error();
            if adapter.is_some() && matches!(e, ReceiverError::None | ReceiverError::AnnounceFailed)
            {
                self.restart_receiver("Netzwerk verfügbar");
            }
            return;
        }
        if adapter_changed(self.receiver.adapter(), adapter.as_ref()) {
            self.restart_receiver("Netzwerk hat sich geändert");
        }
    }

    fn change_language(&mut self) {
        apply_language(&self.config);
        self.update_title();
    }

    fn show_toast(&mut self, text: &str) {
        self.ui.toast = text.to_string();
        self.ui.toast_until_ticks = ticks() + TOAST_MS;
    }

    fn apply_settings(&mut self, next: Config) {
        let mut clean = next;
        if clean.receiver_name.trim_matches([' ', '\t']).is_empty() {
            clean.receiver_name = default_config().receiver_name;
        }
        let effects = diff_config(&self.config, &clean);
        self.config = clean.clone();
        self.ui.draft = clean;
        if effects.contains(&ApplyEffect::ChangeLanguage) {
            self.change_language();
        }
        if save_config(&self.config, &config_file()) {
            self.show_toast(tr(Str::ToastSettingsApplied));
        } else {
            self.show_toast(tr(Str::ToastSettingsSaveFailed));
        }
        for e in &effects {
            match e {
                ApplyEffect::ReconfigureScheduler => self.apply_mode(),
                ApplyEffect::ToggleStats => self.ui.show_stats = self.config.stats_overlay,
                ApplyEffect::ChangeLanguage => {} // schon oben angewendet, damit der Toast passt
                ApplyEffect::RestartReceiver => self.restart_receiver("Einstellungen geändert"),
            }
        }
        let restarted = effects.contains(&ApplyEffect::RestartReceiver);
        if !restarted && !self.receiver.running() {
            self.restart_receiver("Übernehmen bei gestopptem Empfänger");
        }
        log_msg!(
            LogLevel::Info,
            "app: Einstellungen übernommen ({} Änderungen)",
            effects.len()
        );
    }

    fn apply_mode(&mut self) {
        self.video_sched
            .configure(self.config.mode, self.config.smooth_delay_ms);
        self.audio
            .configure(self.config.mode, self.config.smooth_delay_ms);
        log_msg!(
            LogLevel::Info,
            "app: Modus {} ({} ms)",
            if self.config.mode == Mode::Smooth {
                "Flüssig"
            } else {
                "Niedrige Latenz"
            },
            self.config.smooth_delay_ms
        );
    }

    fn start_receiver(&mut self) {
        self.receiver.stop();
        self.ui.receiver_name = self.config.receiver_name.clone();
        self.ui.ips.clear();
        let Some(adapter) = pick_adapter(&enumerate_adapters(), &self.config.adapter) else {
            self.ui.notice = Some(if self.config.adapter == "auto" {
                Str::NoticeNoNetwork
            } else {
                Str::NoticeAdapterUnavailable
            });
            self.ui.notice_arg = self.config.adapter.clone();
            log_msg!(
                LogLevel::Error,
                "app: kein Adapter ({})",
                self.config.adapter
            );
            return;
        };
        self.ui.ips = adapter.ipv4.clone();
        let opts = ReceiverOptions {
            name: self.config.receiver_name.clone(),
            adapter,
            allow_h265: self.config.allow_h265,
            verbose_log: false,
        };
        if !self.receiver.start(&opts) {
            self.ui.notice = Some(match self.receiver.last_error() {
                ReceiverError::PortInUse => Str::NoticePortInUse,
                ReceiverError::AnnounceFailed => Str::NoticeAnnounceFailed,
                _ => Str::NoticeStartFailed,
            });
            return;
        }
        self.ui.notice = None;
    }

    /// Wie eine Verbindung vom Gerät "Demo": Spiegeln-Ansicht, Statistik, F9/F10 wie gewohnt.
    fn start_demo(&mut self) {
        self.ui.receiver_name = self.config.receiver_name.clone();
        self.ui.ips.clear();
        self.ui.notice = None;
        let connected = SessionEvent::Connected {
            device: "Demo".to_string(),
        };
        let actions = self.session.on(&connected, self.ui.recording);
        self.apply_session_actions(&actions);
        log_msg!(LogLevel::Info, "app: Demo-Modus gestartet (Gerät Demo)");
    }

    fn update_title(&self) {
        let mut title = tr(Str::WindowTitle).to_string();
        if self.session.state() == SessionState::Mirroring && !self.session.device().is_empty() {
            title.push_str(" · ");
            title.push_str(self.session.device());
        }
        self.window.set_title(&title);
    }

    fn take_screenshot(&mut self) {
        let Some(shown) = self.last_shown.as_ref() else {
            self.show_toast(tr(Str::ToastNoFrame));
            return;
        };
        if self.screenshot_pending.is_some() {
            self.show_toast(tr(Str::ToastScreenshotBusy));
            return;
        }
        let file = unique_path(
            &self.config.screenshot_dir,
            &timestamp_name("AirPlay", &LocalTime::now(), "png"),
        );
        // Referenz auf das Bild (GPU-Fläche bleibt gültig); Kopieren und Kodieren im Hintergrund.
        let frame = shown.clone_ref();
        self.screenshot_pending =
            Background::spawn("screenshot", move || save_screenshot(&frame, &file));
        if self.screenshot_pending.is_none() {
            self.show_toast(tr(Str::ToastScreenshotFailed));
        }
    }

    fn toggle_recording(&mut self) {
        if self.ui.recording {
            self.stop_recording();
            return;
        }
        if self.session.state() != SessionState::Mirroring {
            self.show_toast(tr(Str::ToastNoConnection));
            return;
        }
        if self.recorder_stopping.is_some() {
            self.show_toast(tr(Str::ToastPreviousRecordingBusy));
            return;
        }
        // Häufigster vom Nutzer behebbarer Fehler: Ordner/Laufwerk fehlt → mit Pfad melden.
        let dir = self.config.record_dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        if !dir.is_dir() {
            self.show_toast(&trf(Str::ToastRecordFolderUnavailable, &[&dir.display()]));
            return;
        }
        let format = VideoFormat {
            width: self.decoder.width(),
            height: self.decoder.height(),
            h265: self.decoder.is_h265(),
        };
        if !self.recorder.start(&dir, format) {
            log_msg!(
                LogLevel::Error,
                "app: Aufnahme nicht möglich: {}",
                self.recorder.last_error()
            );
            self.show_toast(tr(Str::ToastRecordingImpossible));
            return;
        }
        self.ui.recording = true;
        self.ui.record_waiting = self.recorder.waiting_for_keyframe();
        self.recording_audio.store(true, Ordering::SeqCst);
        self.record_start_ticks = ticks();
        self.last_record_check_ticks = self.record_start_ticks;
        self.show_toast(tr(Str::ToastRecordingStarted));
    }

    fn stop_recording(&mut self) {
        if !self.ui.recording {
            return;
        }
        self.ui.recording = false;
        self.recording_audio.store(false, Ordering::SeqCst);
        self.ui.record_waiting = false;
        // Abschließen (inkl. faststart-Umschreiben) kann bei langen Aufnahmen dauern → Hintergrund.
        let recorder = self.recorder.clone();
        self.recorder_stopping = Background::spawn("recorder-stop", move || {
            recorder.stop();
            let files = recorder.files();
            let err = recorder.last_error();
            if !err.is_empty() {
                log_msg!(LogLevel::Error, "app: Aufnahme fehlgeschlagen: {}", err);
                return tr(Str::ToastRecordingFailed).to_string();
            }
            if files.is_empty() {
                return tr(Str::ToastNothingRecorded).to_string();
            }
            if files.len() > 1 {
                return trf(Str::ToastRecordingSavedParts, &[&files.len()]);
            }
            tr(Str::ToastRecordingSaved).to_string()
        });
        if self.recorder_stopping.is_none() {
            // Kein Hintergrund-Thread: hier abschließen.
            self.recorder.stop();
        }
        self.show_toast(tr(Str::ToastRecordingSaving));
    }

    fn reset_media(&mut self) {
        self.decoder.close();
        if let Some(r) = self.renderer.as_mut() {
            r.clear_video();
        }
        self.receiver.video().clear();
        self.frame_ring.clear();
        self.last_shown = None;
        self.last_decoded = (0, 0);
        self.video_sched.reset();
        self.video_lead_est.reset();
        self.audio.request_reset();
        self.receiver.audio().clear();
    }

    fn decode_video(&mut self) {
        for p in self.receiver.video().drain() {
            // auch ohne Aufnahme: hält Pakete ab dem letzten Keyframe vor
            self.recorder.push_video(&p);
            let arrival = if p.arrival_ns != 0 {
                p.arrival_ns
            } else {
                self.receiver.now_ns()
            };
            self.video_lead_est
                .add((p.ntp_local as i64).wrapping_sub(arrival as i64));
            if !self.decoder.is_open() || self.decoder.is_h265() != p.h265 {
                self.decoder
                    .open(self.renderer.as_ref().map(Renderer::device), p.h265);
            }
            for f in self.decoder.decode(&p) {
                self.frames_this_second += 1;
                if (f.width(), f.height()) != self.last_decoded {
                    self.last_decoded = (f.width(), f.height());
                    if self.ui.recording {
                        self.recorder.on_format_change(VideoFormat {
                            width: f.width(),
                            height: f.height(),
                            h265: p.h265,
                        });
                    }
                }
                let ts = f
                    .pts()
                    .map_or_else(|| self.receiver.now_ns(), |pts| (pts as u64) * 1000);
                let id = self.next_frame_id;
                self.next_frame_id += 1;
                self.video_sched.push(ts, id);
                self.frame_ring.push_back((id, f));
                if self.frame_ring.len() > MAX_PENDING_FRAMES {
                    self.frame_ring.pop_front();
                    self.ring_dropped += 1;
                }
            }
        }
    }

    /// Bild nach `FrameScheduler`; "Flüssig" nutzt denselben Versatz wie der Ton-Thread
    /// (Gesamtverzögerung ab Ankunft).
    fn present_video(&mut self) {
        if self.config.mode == Mode::Smooth {
            if self.video_lead_est.known() {
                self.audio.set_video_lead_ns(self.video_lead_est.median());
            }
            self.video_sched
                .set_present_offset(self.audio.present_offset_ns());
        }
        let Some(picked) = self.video_sched.pick(self.receiver.now_ns()) else {
            return;
        };
        while self.frame_ring.front().is_some_and(|(id, _)| *id < picked) {
            self.frame_ring.pop_front();
        }
        if self.frame_ring.front().is_some_and(|(id, _)| *id == picked)
            && let Some((_, f)) = self.frame_ring.pop_front()
        {
            if let Some(r) = self.renderer.as_mut() {
                r.upload_frame(&f);
            }
            self.last_shown = Some(f);
        }
    }

    fn update_stats(&mut self) {
        let now = ticks();
        let elapsed = now.saturating_sub(self.last_stats_tick);
        if elapsed < 1000 {
            return;
        }
        let secs = elapsed as f32 / 1000.0;
        self.last_stats_tick = now;
        self.ui.fps = self.frames_this_second as f32 / secs;
        self.frames_this_second = 0;
        self.ui.width = self.decoder.width();
        self.ui.height = self.decoder.height();
        self.ui.codec = if self.decoder.is_h265() {
            "H.265"
        } else {
            "H.264"
        };
        self.ui.hw = !self.decoder.is_open() || self.decoder.hardware();
        self.ui.queue_depth = self.receiver.video().len();
        self.ui.dropped_frames =
            self.receiver.video().dropped() + self.video_sched.dropped() + self.ring_dropped;
        self.ui.mode = self.config.mode;
        self.ui.audio_buffer_ms = self.audio.stats().buffer_ms;
    }

    fn frame(&mut self) {
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        r.begin_frame();
        if self.session.state() == SessionState::Mirroring && r.has_video() {
            r.draw_video(letterbox(
                r.video_width(),
                r.video_height(),
                r.width(),
                r.height(),
            ));
        }
        self.ui.now_ticks = ticks();
        self.ui.record_seconds = if self.ui.recording {
            (self.ui.now_ticks.saturating_sub(self.record_start_ticks) / 1000) as i32
        } else {
            0
        };

        // Oberfläche über dem Videobild, im selben Durchgang vor Present
        let size = [r.width().max(0) as u32, r.height().max(0) as u32];
        let raw = self.egui_input.take_raw_input(
            size,
            self.window.display_scale(),
            self.ui.now_ticks as f64 / 1000.0,
        );
        let ui = &mut self.ui;
        let mut out = self.egui_ctx.run_pass(raw, |ctx| draw_ui(ctx, ui));
        let wants_keyboard = egui_wants_text(&self.egui_ctx);
        self.ui.wants_keyboard = wants_keyboard;
        self.egui_input
            .set_text_input(self.window.raw(), wants_keyboard);
        self.egui_input.handle_output(&out.platform_output);
        let prims = self
            .egui_ctx
            .tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        match self.painter.as_mut() {
            Some(p) => p.paint(
                r.context(),
                size,
                out.pixels_per_point,
                &prims,
                &mut out.textures_delta,
            ),
            None => out.textures_delta.clear(),
        }

        if !r.end_frame() {
            log_msg!(LogLevel::Warn, "app: baue Grafik neu auf");
            self.last_shown = None;
            self.frame_ring.clear();
            self.video_sched.reset();
            self.decoder.close();
            self.painter = None;
            self.renderer = None;
            match Renderer::init(self.window.raw()) {
                Ok(r) => {
                    self.painter = EguiPainter::new(r.device()).ok();
                    // Neuer Kontext, damit egui die Schrift-Textur für den neuen Painter
                    // vollständig neu schickt.
                    self.egui_ctx = new_egui_context();
                    self.renderer = Some(r);
                }
                Err(_) => self.running = false,
            }
        }
    }

    /// Reihenfolge wie v1; mehrfacher Aufruf ist harmlos. Fenster und SDL gibt danach `Drop` frei.
    fn shutdown(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        if self.ui.recording {
            self.ui.recording = false;
            self.recording_audio.store(false, Ordering::SeqCst);
            self.recorder.stop();
        }
        if let Some(t) = self.recorder_stopping.take() {
            t.wait();
        }
        if let Some(t) = self.screenshot_pending.take() {
            t.wait();
        }
        if let Some(t) = self.firewall_pending.take() {
            t.wait();
        }
        if let Some(t) = self.firewall_check.take() {
            t.wait();
        }
        self.receiver.stop();
        self.audio.stop();
        self.audio.set_pcm_tap(None);
        self.last_shown = None;
        self.frame_ring.clear();
        self.decoder.close();
        self.audio_out.close();
        self.renderer = None;
        log_msg!(LogLevel::Info, "app: beendet");
        log_shutdown();
    }
}

/// Setzt die Sperre auch bei einer Panik zurück.
struct BusyGuard<'a>(&'a Cell<bool>);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Event-Watch für die Lebensdauer von `run`; wird auch bei einer Panik wieder entfernt.
struct EventWatch(*mut c_void);

impl EventWatch {
    fn add(userdata: *mut c_void) -> Self {
        // SAFETY: userdata (die App) lebt, bis dieser Wächter fallen gelassen wird.
        unsafe { SDL_AddEventWatch(Some(live_redraw), userdata) };
        Self(userdata)
    }
}

impl Drop for EventWatch {
    fn drop(&mut self) {
        // SAFETY: dieselbe Kombination wie bei SDL_AddEventWatch.
        unsafe { SDL_RemoveEventWatch(Some(live_redraw), self.0) };
    }
}

/// Die Anwendung. Der Zustand liegt in einer `UnsafeCell` mit Sperre gegen verschachteltes
/// Zeichnen: Die Event-Watch kann aus `SDL_PollEvent` heraus aufgerufen werden; `with` gibt
/// den Zustand nur heraus, wenn er gerade nicht benutzt wird. Nur auf dem Haupt-Thread.
pub struct App {
    busy: Cell<bool>,
    /// während Verschieben/Größe ändern gezeichnet (Diagnose)
    live_frames: Cell<u64>,
    main_thread: ThreadId,
    inner: UnsafeCell<Inner>,
}

impl App {
    /// Log, Einstellungen, Sprache, Fenster, Renderer, Ton und Empfänger starten.
    pub fn new() -> Result<App, String> {
        Self::new_with(Options::default())
    }

    /// Wie `new`; mit `options.demo` statt des Empfängers die Demo-Datei abspielen.
    pub fn new_with(options: Options) -> Result<App, String> {
        init_log();
        install_panic_log();
        log_msg!(
            LogLevel::Info,
            "AirPlay-Empfänger {} startet",
            env!("CARGO_PKG_VERSION")
        );
        let demo = match options.demo.as_deref() {
            None => None,
            Some(path) => match DemoSource::open(path) {
                Ok(d) => {
                    log_msg!(LogLevel::Info, "app: Demo-Modus mit {}", path.display());
                    Some(d)
                }
                Err(e) => {
                    log_msg!(LogLevel::Error, "app: {}", e);
                    log_msg!(LogLevel::Info, "app: beendet");
                    log_shutdown();
                    return Err(e);
                }
            },
        };
        let cfg_file = config_file();
        let config = load_config(&cfg_file);
        if !cfg_file.exists() {
            save_config(&config, &cfg_file);
        }
        match Inner::init(config, demo) {
            Ok(inner) => Ok(App {
                busy: Cell::new(false),
                live_frames: Cell::new(0),
                main_thread: std::thread::current().id(),
                inner: UnsafeCell::new(inner),
            }),
            Err(e) => {
                log_msg!(LogLevel::Info, "app: beendet");
                log_shutdown();
                Err(e)
            }
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> Option<R> {
        if self.busy.replace(true) {
            return None;
        }
        let _guard = BusyGuard(&self.busy);
        // SAFETY: `busy` stellt sicher, dass es höchstens eine &mut-Ausleihe gibt; App wird nur
        // auf dem Haupt-Thread benutzt (kein Send/Sync wegen Cell/UnsafeCell und Rohzeigern).
        Some(f(unsafe { &mut *self.inner.get() }))
    }

    /// Hauptschleife bis zum Schließen des Fensters; danach ist alles freigegeben.
    pub fn run(&mut self) {
        let this: &App = self;
        let watch = EventWatch::add((this as *const App).cast_mut().cast::<c_void>());
        loop {
            let live_before = this.live_frames.get();
            while let Some(e) = poll_event() {
                this.with(|s| s.handle_event(&e));
            }
            let live = this.live_frames.get() - live_before;
            if live > 0 {
                log_msg!(
                    LogLevel::Debug,
                    "app: {} Bilder während Verschieben/Größe ändern gezeichnet",
                    live
                );
            }
            let Some((running, minimized)) = this.with(|s| (s.running, s.window.minimized()))
            else {
                break;
            };
            if !running {
                break;
            }
            if minimized {
                this.with(Inner::handle_session);
                // SAFETY: keine Vorbedingungen.
                unsafe { SDL_Delay(20) };
                continue;
            }
            this.with(Inner::tick);
        }
        drop(watch);
        this.with(Inner::shutdown);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.inner.get_mut().shutdown();
    }
}

fn init_log() {
    if let Some(dir) = known_dir(KnownDir::LocalAppData) {
        log_init(&dir.join("airplay").join("logs"));
    }
}

/// Fehler vor dem Start (z. B. falsche Befehlszeile) ins Log schreiben; das Programm hat keine
/// Konsole.
pub fn log_startup_error(msg: &str) {
    init_log();
    log_msg!(LogLevel::Error, "app: Befehlszeile: {}", msg);
    log_shutdown();
}

/// Paniken (auch in anderen Threads) landen im Log; das Programm hat keine Konsole.
fn install_panic_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log_msg!(LogLevel::Error, "Panik: {}", info);
        previous(info);
    }));
}

/// Event-Watch: zeichnet während Verschieben/Größe ändern weiter. Darf nicht paniken.
unsafe extern "C" fn live_redraw(userdata: *mut c_void, e: *mut SDL_Event) -> bool {
    let r = catch_unwind(AssertUnwindSafe(|| {
        if userdata.is_null() || e.is_null() {
            return;
        }
        // SAFETY: userdata ist die App aus `run`, die bis nach SDL_RemoveEventWatch lebt;
        // e ist ein gültiges Ereignis.
        let (app, ev) = unsafe { (&*(userdata as *const App), &*e) };
        // SAFETY: Typfeld steht in jedem Ereignis.
        let ty = unsafe { ev.r#type };
        let size_changed = ty == SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED.0;
        let exposed = ty == SDL_EVENT_WINDOW_EXPOSED.0;
        if !(size_changed || exposed) || std::thread::current().id() != app.main_thread {
            return;
        }
        // SAFETY: Fensterereignis.
        let w = unsafe { ev.window };
        app.with(|s| {
            if w.windowID != s.window.id() {
                return;
            }
            if size_changed {
                s.resize(w.data1, w.data2);
            } else if w.data1 == 1 {
                app.live_frames.set(app.live_frames.get() + 1);
                s.tick();
            }
        });
    }));
    if r.is_err() {
        log_msg!(
            LogLevel::Error,
            "app: Panik beim Zeichnen während Verschieben/Größe ändern abgefangen"
        );
    }
    true
}
