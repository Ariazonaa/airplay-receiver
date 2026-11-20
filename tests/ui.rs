//! Oberfläche ohne Fenster: Zahnrad fordert die Einstellungen an (die App füllt `draft`, bevor
//! das Fenster erscheint), alle Bildschirme zeichnen sich in beiden Sprachen.

use airplay::app::config::default_config;
use airplay::app::egui_input::egui_wants_text;
use airplay::app::hotkeys::{Command, hotkey_command};
use airplay::app::session_controller::SessionState;
use airplay::app::strings::{Lang, Str, set_language};
use airplay::app::ui::{UiAction, UiState, draw_ui, init_ui_fonts};
use airplay::ffi::sdl3::keycode::{SDL_KMOD_LCTRL, SDL_KMOD_NONE, SDLK_F9, SDLK_F10, SDLK_T};
use egui::{Event, Id, Key, LayerId, Modifiers, Order, PointerButton, Pos2, Rect, pos2, vec2};

fn raw(events: Vec<Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
        events,
        ..Default::default()
    }
}

/// Ein Durchlauf; die Texturänderungen braucht ohne Painter niemand.
fn frame(ctx: &egui::Context, s: &mut UiState, events: Vec<Event>) -> egui::FullOutput {
    let mut out = ctx.run_pass(raw(events), |ctx| draw_ui(ctx, s));
    out.textures_delta.clear();
    out
}

fn click(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn gear_click_requests_settings_without_opening_stale_draft() {
    let ctx = egui::Context::default();
    init_ui_fonts(&ctx);
    let mut s = UiState::new(&default_config());
    s.session = SessionState::Idle;
    // Erster Durchlauf misst nur (egui-Größenbestimmung), der zweite legt die Lage fest.
    let _ = frame(&ctx, &mut s, vec![]);
    let _ = frame(&ctx, &mut s, vec![]);
    let gear = ctx
        .memory(|m| m.area_rect(Id::unique("gear")))
        .expect("Zahnrad gezeichnet");
    assert!(
        gear.max.x > 1200.0 && gear.min.y < 60.0,
        "oben rechts: {gear:?}"
    );
    let c = gear.center();
    let _ = frame(&ctx, &mut s, vec![Event::PointerMoved(c), click(c, true)]);
    let _ = frame(&ctx, &mut s, vec![click(c, false)]);
    assert_eq!(s.actions, vec![UiAction::OpenSettings]);
    assert!(
        !s.settings_open,
        "erst die App öffnet das Fenster (nach dem Füllen)"
    );
}

#[test]
fn all_screens_draw_in_both_languages() {
    for lang in [Lang::De, Lang::En] {
        set_language(lang);
        let ctx = egui::Context::default();
        init_ui_fonts(&ctx);
        let mut s = UiState::new(&default_config());
        s.ips = vec!["192.168.1.10".into(), "10.0.0.2".into()];
        s.firewall_missing = true;
        s.firewall_note = Some(Str::FirewallDeclined);
        s.notice = Some(Str::NoticeAdapterUnavailable);
        s.notice_arg = "WLAN Ä".into();
        s.show_stats = true;
        s.recording = true;
        s.record_seconds = 75;
        s.toast = "Einstellungen übernommen".into();
        s.toast_until_ticks = u64::MAX;
        s.settings_open = true;
        s.adapters = vec![("Ethernet".into(), false), ("vEthernet".into(), true)];
        for _ in 0..3 {
            let out = frame(&ctx, &mut s, vec![]);
            assert!(!out.shapes.is_empty());
        }
        for id in ["notices", "toast", "rec", "settings"] {
            assert!(
                ctx.memory(|m| m.area_rect(Id::unique(id))).is_some(),
                "{id} fehlt ({lang:?})"
            );
        }
        // Spiegeln: kein Firewall-Hinweis über dem Bild, nur der dauerhafte Hinweis bleibt.
        s.notice = None;
        s.session = SessionState::Mirroring;
        let _ = frame(&ctx, &mut s, vec![]);
        let _ = frame(&ctx, &mut s, vec![]);
        assert!(
            !ctx.memory(|m| m
                .areas()
                .visible_last_frame(&LayerId::new(Order::Middle, Id::unique("notices")))),
            "Firewall-Hinweis beim Spiegeln ({lang:?})"
        );
    }
    set_language(Lang::De);
}

fn tab(pressed: bool) -> Event {
    Event::Key {
        key: Key::Tab,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

/// Kürzel, die die App bei diesem egui-Zustand auslösen würde.
fn hotkeys_now(ctx: &egui::Context) -> Vec<Option<Command>> {
    let typing = egui_wants_text(ctx);
    vec![
        hotkey_command(SDLK_F9, SDL_KMOD_NONE, false, typing),
        hotkey_command(SDLK_F10, SDL_KMOD_NONE, false, typing),
        hotkey_command(SDLK_T, SDL_KMOD_LCTRL, false, typing),
    ]
}

#[test]
fn tab_focus_on_button_keeps_hotkeys_text_field_blocks_them() {
    let ctx = egui::Context::default();
    init_ui_fonts(&ctx);
    let mut s = UiState::new(&default_config());
    s.session = SessionState::Idle;
    let _ = frame(&ctx, &mut s, vec![]);
    let _ = frame(&ctx, &mut s, vec![]);
    // Tab ohne Textfeld: egui fokussiert einen Knopf (Zahnrad) – Kürzel bleiben aktiv.
    let _ = frame(&ctx, &mut s, vec![tab(true), tab(false)]);
    let _ = frame(&ctx, &mut s, vec![]);
    assert!(
        ctx.memory(|m| m.focused()).is_some(),
        "Tab sollte einen Knopf fokussieren"
    );
    assert!(!egui_wants_text(&ctx), "Knopf-Fokus ist keine Texteingabe");
    assert_eq!(
        hotkeys_now(&ctx),
        vec![
            Some(Command::Screenshot),
            Some(Command::ToggleRecording),
            Some(Command::ToggleOnTop)
        ]
    );

    // Einstellungen offen, per Tab bis ins Namensfeld: dort wird getippt, keine Kürzel.
    s.settings_open = true;
    let _ = frame(&ctx, &mut s, vec![]);
    let _ = frame(&ctx, &mut s, vec![]);
    let mut tabs = 0;
    while !ctx.text_edit_focused() && tabs < 20 {
        let _ = frame(&ctx, &mut s, vec![tab(true), tab(false)]);
        let _ = frame(&ctx, &mut s, vec![]);
        tabs += 1;
    }
    assert!(ctx.text_edit_focused(), "kein Textfeld per Tab erreicht");
    assert!(egui_wants_text(&ctx));
    assert_eq!(hotkeys_now(&ctx), vec![None, None, None]);
}
