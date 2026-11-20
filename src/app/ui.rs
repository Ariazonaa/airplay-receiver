//! Oberfläche mit egui (v1 `ui.cpp`): Wartebildschirm, Hinweise, Statistik, Zahnrad,
//! Einstellungsfenster, REC-Anzeige und Einblendungen. Liest `UiState`, meldet Wünsche über
//! `UiState::actions` (v1: `UiCallbacks`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{
    Align2, Color32, ComboBox, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame,
    Grid, Id, Label, LayerId, Margin, Order, RichText, Slider, TextEdit, TextWrapMode, pos2, vec2,
};

use crate::app::config::{Config, Mode, SMOOTH_DELAY_MAX_MS, SMOOTH_DELAY_MIN_MS};
use crate::app::log::LogLevel;
use crate::app::session_controller::SessionState;
use crate::app::strings::{Str, tr, trf};
use crate::log_msg;

/// Wünsche der Oberfläche an die App (v1: `UiCallbacks`), abgearbeitet in `tick`.
#[derive(Clone, Debug, PartialEq)]
pub enum UiAction {
    AddFirewallRule,
    /// füllt `draft` + `adapters`
    OpenSettings,
    ApplySettings(Config),
    OpenFolder(PathBuf),
}

/// Zustand für die Oberfläche (v1 `UiState`); die App schreibt, die Oberfläche zeichnet.
#[derive(Clone, Debug)]
pub struct UiState {
    pub receiver_name: String,
    pub ips: Vec<String>,
    pub session: SessionState,
    pub device: String,
    pub recording: bool,
    /// Aufnahme wartet noch auf ein vollständiges Bild
    pub record_waiting: bool,
    pub record_seconds: i32,
    pub firewall_missing: bool,
    /// Ergebnis des letzten Versuchs (beim Zeichnen übersetzt)
    pub firewall_note: Option<Str>,
    /// dauerhafter Hinweis (z. B. kein Netzwerkadapter)
    pub notice: Option<Str>,
    /// Platzhalter für `notice` (z. B. Adaptername)
    pub notice_arg: String,
    pub show_stats: bool,
    // Statistik (Overlay unten links)
    pub fps: f32,
    pub width: i32,
    pub height: i32,
    pub codec: &'static str,
    pub hw: bool,
    pub queue_depth: usize,
    pub dropped_frames: u64,
    pub mode: Mode,
    pub audio_buffer_ms: f32,
    // Zahnrad + Einstellungen
    pub now_ticks: u64,
    pub last_mouse_move_ticks: u64,
    pub settings_open: bool,
    /// Arbeitskopie im Einstellungsfenster
    pub draft: Config,
    /// Name, virtuell
    pub adapters: Vec<(String, bool)>,
    /// kurze Einblendung
    pub toast: String,
    pub toast_until_ticks: u64,
    /// Ein egui-Textfeld hat den Fokus (`egui_wants_text`) → keine Tastenkürzel.
    pub wants_keyboard: bool,
    /// Von der Oberfläche gemeldet, von der App im nächsten `tick` abgearbeitet.
    pub actions: Vec<UiAction>,
}

impl UiState {
    pub fn new(config: &Config) -> Self {
        Self {
            receiver_name: config.receiver_name.clone(),
            ips: Vec::new(),
            session: SessionState::Idle,
            device: String::new(),
            recording: false,
            record_waiting: false,
            record_seconds: 0,
            firewall_missing: false,
            firewall_note: None,
            notice: None,
            notice_arg: String::new(),
            show_stats: config.stats_overlay,
            fps: 0.0,
            width: 0,
            height: 0,
            codec: "",
            hw: true,
            queue_depth: 0,
            dropped_frames: 0,
            mode: config.mode,
            audio_buffer_ms: 0.0,
            now_ticks: 0,
            last_mouse_move_ticks: 0,
            settings_open: false,
            draft: config.clone(),
            adapters: Vec::new(),
            toast: String::new(),
            toast_until_ticks: 0,
            wants_keyboard: false,
            actions: Vec::new(),
        }
    }
}

/// Zahnrad bleibt so lange nach der letzten Mausbewegung sichtbar.
const GEAR_VISIBLE_MS: u64 = 2000;
const FONT_DIR: &str = r"C:\Windows\Fonts";
/// Schriftfamilie für den großen Empfängernamen
const TITLE_FAMILY: &str = "title";
const BODY_SIZE: f32 = 17.0;
const TITLE_SIZE: f32 = 44.0;
const MARGIN: f32 = 12.0;
/// Inhaltsbreite des Einstellungsfensters (Beschriftung + Feld + „Öffnen“)
const SETTINGS_WIDTH: f32 = 540.0;

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

const TEXT_TITLE: Color32 = rgba(235, 240, 245, 255);
const TEXT_DIM: Color32 = rgba(150, 165, 178, 255);
const TEXT_IPS: Color32 = rgba(100, 115, 128, 255);
const TEXT_WARN: Color32 = rgba(226, 167, 76, 255);
const WINDOW_BG: Color32 = rgba(23, 28, 33, 240);
const REC_BG: Color32 = rgba(192, 57, 43, 230);

fn load_font(file: &str) -> Option<Vec<u8>> {
    std::fs::read(Path::new(FONT_DIR).join(file)).ok()
}

/// Segoe UI (Umlaute) mit Segoe UI Symbol als Rückfall (●, ⚙), Titel fett; fehlt Segoe UI,
/// bleibt die egui-Standardschrift. Dazu der dunkle Stil wie v1.
pub fn init_ui_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let defaults = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut body = Vec::new();
    let mut title = Vec::new();
    if let Some(regular) = load_font("segoeui.ttf") {
        fonts
            .font_data
            .insert("segoeui".into(), Arc::new(FontData::from_owned(regular)));
        body.push("segoeui".to_string());
        if let Some(bold) = load_font("segoeuib.ttf") {
            fonts
                .font_data
                .insert("segoeuib".into(), Arc::new(FontData::from_owned(bold)));
            title.push("segoeuib".to_string());
        } else {
            title.push("segoeui".to_string());
        }
        if let Some(sym) = load_font("seguisym.ttf") {
            fonts
                .font_data
                .insert("seguisym".into(), Arc::new(FontData::from_owned(sym)));
            body.push("seguisym".to_string());
            title.push("seguisym".to_string());
        }
        log_msg!(LogLevel::Info, "ui: Schrift Segoe UI");
    } else {
        log_msg!(
            LogLevel::Warn,
            "ui: Segoe UI fehlt, nehme die egui-Standardschrift"
        );
    }
    body.extend(defaults.iter().cloned());
    title.extend(defaults);
    fonts.families.insert(FontFamily::Proportional, body);
    fonts
        .families
        .insert(FontFamily::Name(TITLE_FAMILY.into()), title);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        use egui::TextStyle;
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(13.0)),
            (TextStyle::Body, FontId::proportional(BODY_SIZE)),
            (TextStyle::Button, FontId::proportional(BODY_SIZE)),
            (TextStyle::Heading, FontId::proportional(22.0)),
            (TextStyle::Monospace, FontId::monospace(15.0)),
        ]
        .into();
        style.spacing.window_margin = Margin::symmetric(14, 12);
        style.spacing.button_padding = vec2(10.0, 6.0);
        style.spacing.item_spacing = vec2(8.0, 7.0);
        let v = &mut style.visuals;
        v.window_corner_radius = CornerRadius::same(8);
        v.window_fill = WINDOW_BG;
        v.panel_fill = WINDOW_BG;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::same(6);
        }
        // Knöpfe petrol wie v1
        v.widgets.inactive.weak_bg_fill = rgba(10, 122, 138, 255);
        v.widgets.hovered.weak_bg_fill = rgba(18, 148, 166, 255);
        v.widgets.active.weak_bg_fill = rgba(8, 102, 115, 255);
    });
}

/// Rahmen wie ein Fenster, Hintergrund mit eigener Deckkraft.
fn panel_frame(ui: &egui::Ui, fill: Color32) -> Frame {
    Frame::window(ui.style()).fill(fill)
}

fn draw_idle(ctx: &egui::Context, s: &UiState) {
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(LayerId::background());
    let block = TITLE_SIZE + BODY_SIZE * 3.5;
    let mut y = screen.min.y + (screen.height() - block) * 0.45;
    let x = screen.center().x;
    painter.text(
        pos2(x, y),
        Align2::CENTER_TOP,
        &s.receiver_name,
        FontId::new(TITLE_SIZE, FontFamily::Name(TITLE_FAMILY.into())),
        TEXT_TITLE,
    );
    y += TITLE_SIZE * 1.3 + BODY_SIZE * 0.4;
    painter.text(
        pos2(x, y),
        Align2::CENTER_TOP,
        tr(Str::WaitingForConnection),
        FontId::proportional(BODY_SIZE),
        TEXT_DIM,
    );
    y += BODY_SIZE * 1.3 + BODY_SIZE * 0.8;
    if !s.ips.is_empty() {
        painter.text(
            pos2(x, y),
            Align2::CENTER_TOP,
            s.ips.join("  ·  "),
            FontId::proportional(BODY_SIZE),
            TEXT_IPS,
        );
    }
}

fn draw_notices(ctx: &egui::Context, s: &mut UiState) {
    // Beim Spiegeln ist die Verbindung offensichtlich da: Firewall-Hinweis nicht übers Bild legen.
    let show_firewall = s.firewall_missing && s.session != SessionState::Mirroring;
    if !show_firewall && s.notice.is_none() {
        return;
    }
    let max_w = (ctx.content_rect().width() - 2.0 * 16.0 - 28.0).max(100.0);
    egui::Area::new(Id::unique("notices"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -16.0))
        .show(ctx, |ui| {
            panel_frame(ui, WINDOW_BG).show(ui, |ui| {
                ui.set_max_width(max_w);
                if let Some(n) = s.notice {
                    ui.add(Label::new(trf(n, &[&s.notice_arg])).wrap());
                }
                if show_firewall {
                    ui.add(Label::new(tr(Str::FirewallBlocks)).wrap());
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add(
                                egui::Button::new(tr(Str::FirewallAddRule))
                                    .wrap_mode(TextWrapMode::Extend),
                            )
                            .clicked()
                        {
                            s.actions.push(UiAction::AddFirewallRule);
                        }
                        if let Some(n) = s.firewall_note {
                            ui.label(tr(n));
                        }
                    });
                }
            });
        });
}

fn draw_stats(ctx: &egui::Context, s: &UiState) {
    let mirroring = s.session == SessionState::Mirroring;
    let sw_hint = mirroring && !s.hw;
    if !(s.show_stats && mirroring) && !sw_hint {
        return;
    }
    egui::Area::new(Id::unique("stats"))
        .anchor(Align2::LEFT_BOTTOM, vec2(MARGIN, -MARGIN))
        .interactable(false)
        .show(ctx, |ui| {
            panel_frame(ui, rgba(23, 28, 33, 140)).show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                ui.spacing_mut().item_spacing.y = 2.0;
                if s.show_stats {
                    let hw = tr(if s.hw { Str::HwGpu } else { Str::HwSoftware });
                    let mode = tr(if s.mode == Mode::Smooth {
                        Str::ModeSmooth
                    } else {
                        Str::ModeLowLatency
                    });
                    ui.label(trf(
                        Str::StatsLine1,
                        &[&format!("{:.0}", s.fps), &s.width, &s.height, &s.codec, &hw],
                    ));
                    ui.label(trf(
                        Str::StatsLine2,
                        &[&mode, &s.queue_depth, &s.dropped_frames],
                    ));
                    ui.label(trf(
                        Str::StatsLine3,
                        &[&format!("{:.0}", s.audio_buffer_ms)],
                    ));
                }
                if sw_hint {
                    ui.colored_label(TEXT_WARN, tr(Str::SoftwareDecodeHint));
                }
            });
        });
}

fn draw_gear(ctx: &egui::Context, s: &mut UiState) {
    let recent_mouse = s.now_ticks.saturating_sub(s.last_mouse_move_ticks) < GEAR_VISIBLE_MS;
    if s.settings_open || !(recent_mouse || s.session == SessionState::Idle) {
        return;
    }
    egui::Area::new(Id::unique("gear"))
        .anchor(Align2::RIGHT_TOP, vec2(-MARGIN, MARGIN))
        .show(ctx, |ui| {
            panel_frame(ui, rgba(23, 28, 33, 153)).show(ui, |ui| {
                // Breite nach Sprachwechsel neu bestimmen statt umzubrechen
                ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                // Erst die App öffnet das Fenster, nachdem sie `draft` und `adapters` gefüllt hat.
                if ui.button(tr(Str::SettingsButton)).clicked()
                    && !s.actions.contains(&UiAction::OpenSettings)
                {
                    s.actions.push(UiAction::OpenSettings);
                }
            });
        });
}

/// Abschnittsüberschrift (v1 `SeparatorText`).
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().color(TEXT_TITLE));
    ui.separator();
}

/// Pfad als Textfeld (v1: 512-Byte-Puffer).
fn path_edit(ui: &mut egui::Ui, path: &mut PathBuf) {
    let mut text = path.to_string_lossy().into_owned();
    let changed = ui
        .add(
            TextEdit::singleline(&mut text)
                .char_limit(511)
                .desired_width(260.0),
        )
        .changed();
    if changed {
        *path = PathBuf::from(text);
    }
}

fn settings_grid(id: &str) -> Grid {
    Grid::new(id)
        .num_columns(2)
        .min_col_width(170.0)
        .spacing([16.0, 8.0])
}

/// Inhalt des Einstellungsfensters; true = „Schließen“ gedrückt.
fn settings_contents(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    let mut close = false;
    let adapters = &s.adapters;
    let actions = &mut s.actions;
    let d = &mut s.draft;
    settings_grid("settings_general").show(ui, |ui| {
        ui.label(tr(Str::ReceiverName));
        ui.add(
            TextEdit::singleline(&mut d.receiver_name)
                .char_limit(127)
                .desired_width(280.0),
        );
        ui.end_row();

        ui.label(tr(Str::LanguageLabel));
        let lang_label = match d.language.as_str() {
            "de" => "Deutsch",
            "en" => "English",
            _ => tr(Str::LanguageAuto),
        };
        ComboBox::from_id_salt("settings_language")
            .width(280.0)
            .selected_text(lang_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.language, "auto".into(), tr(Str::LanguageAuto));
                ui.selectable_value(&mut d.language, "de".into(), "Deutsch");
                ui.selectable_value(&mut d.language, "en".into(), "English");
            });
        ui.end_row();
    });

    section(ui, tr(Str::ModeSection));
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.mode, Mode::LowLatency, tr(Str::ModeLowLatency));
        ui.radio_value(&mut d.mode, Mode::Smooth, tr(Str::ModeSmooth));
    });
    settings_grid("settings_mode").show(ui, |ui| {
        ui.label(tr(Str::SmoothDelay));
        ui.add_enabled(
            d.mode == Mode::Smooth,
            Slider::new(
                &mut d.smooth_delay_ms,
                SMOOTH_DELAY_MIN_MS..=SMOOTH_DELAY_MAX_MS,
            )
            .suffix(" ms"),
        );
        ui.end_row();
    });

    section(ui, tr(Str::NetworkSection));
    settings_grid("settings_network").show(ui, |ui| {
        ui.label(tr(Str::NetworkAdapter));
        let current = if d.adapter == "auto" {
            tr(Str::AdapterAuto).to_string()
        } else {
            d.adapter.clone()
        };
        ComboBox::from_id_salt("settings_adapter")
            .width(280.0)
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.adapter, "auto".into(), tr(Str::AdapterAuto));
                for (name, is_virtual) in adapters {
                    let label = if *is_virtual {
                        format!("{name}{}", tr(Str::AdapterVirtualSuffix))
                    } else {
                        name.clone()
                    };
                    ui.selectable_value(&mut d.adapter, name.clone(), label);
                }
            });
        ui.end_row();
    });
    ui.checkbox(&mut d.allow_h265, tr(Str::AllowH265));
    ui.checkbox(&mut d.stats_overlay, tr(Str::ShowStats));

    section(ui, tr(Str::StorageSection));
    settings_grid("settings_storage").show(ui, |ui| {
        ui.label(tr(Str::Screenshots));
        ui.horizontal(|ui| {
            path_edit(ui, &mut d.screenshot_dir);
            if ui.button(tr(Str::Open)).clicked() {
                actions.push(UiAction::OpenFolder(d.screenshot_dir.clone()));
            }
        });
        ui.end_row();
        ui.label(tr(Str::Recordings));
        ui.horizontal(|ui| {
            path_edit(ui, &mut d.record_dir);
            if ui.button(tr(Str::Open)).clicked() {
                actions.push(UiAction::OpenFolder(d.record_dir.clone()));
            }
        });
        ui.end_row();
    });

    ui.add_space(4.0);
    let width = ui.min_rect().width();
    ui.scope(|ui| {
        ui.set_max_width(width);
        ui.add(Label::new(RichText::new(tr(Str::RestartWarning)).color(TEXT_DIM)).wrap());
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button(tr(Str::Apply)).clicked() {
            actions.push(UiAction::ApplySettings(d.clone()));
        }
        if ui.button(tr(Str::Close)).clicked() {
            close = true;
        }
    });
    close
}

fn draw_settings(ctx: &egui::Context, s: &mut UiState) {
    if !s.settings_open {
        return;
    }
    let screen = ctx.content_rect();
    let mut open = true;
    let mut close = false;
    // Feste ID: der Titel wechselt mit der Sprache.
    egui::Window::new(tr(Str::SettingsTitle))
        .id(Id::unique("settings"))
        .open(&mut open)
        .collapsible(false)
        .auto_sized()
        .pivot(Align2::CENTER_CENTER)
        .default_pos(screen.center())
        .max_width((screen.width() - 32.0).max(200.0))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height((screen.height() - 110.0).max(120.0))
                .show(ui, |ui| {
                    // Trennlinien und Zeilen sonst so breit wie das ganze Fenster
                    ui.set_max_width(SETTINGS_WIDTH.min(screen.width() - 64.0).max(200.0));
                    close = settings_contents(ui, s);
                });
        });
    if !open || close {
        s.settings_open = false;
    }
}

fn draw_recording(ctx: &egui::Context, s: &UiState) {
    if !s.recording {
        return;
    }
    let text = if s.record_waiting {
        tr(Str::RecWaiting).to_string()
    } else {
        trf(
            Str::RecTimer,
            &[
                &format!("{:02}", s.record_seconds / 60),
                &format!("{:02}", s.record_seconds % 60),
            ],
        )
    };
    egui::Area::new(Id::unique("rec"))
        .order(Order::Foreground)
        .fixed_pos(pos2(MARGIN, MARGIN))
        .interactable(false)
        .show(ctx, |ui| {
            panel_frame(ui, REC_BG).show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                ui.colored_label(Color32::WHITE, text);
            });
        });
}

fn draw_toast(ctx: &egui::Context, s: &UiState) {
    if s.toast.is_empty() || s.now_ticks > s.toast_until_ticks {
        return;
    }
    egui::Area::new(Id::unique("toast"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 16.0))
        .interactable(false)
        .show(ctx, |ui| {
            panel_frame(ui, rgba(23, 28, 33, 204)).show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                ui.label(&s.toast);
            });
        });
}

/// Ein Bild der Oberfläche (Reihenfolge wie v1).
pub fn draw_ui(ctx: &egui::Context, s: &mut UiState) {
    if s.session == SessionState::Idle {
        draw_idle(ctx, s);
    }
    draw_stats(ctx, s);
    draw_notices(ctx, s);
    draw_gear(ctx, s);
    draw_settings(ctx, s);
    draw_recording(ctx, s);
    draw_toast(ctx, s);
}
