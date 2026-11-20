//! SDL3-Ereignisse → egui-Eingabe (Review-Fokus 5: Umlaute, Backspace, Strg+V).

use std::ffi::CString;
use std::sync::Mutex;

use airplay::app::egui_input::EguiInput;
use airplay::ffi::sdl3::clipboard::SDL_SetClipboardText;
use airplay::ffi::sdl3::events::{
    SDL_EVENT_KEY_DOWN, SDL_EVENT_MOUSE_MOTION, SDL_EVENT_MOUSE_WHEEL, SDL_EVENT_TEXT_INPUT,
    SDL_Event,
};
use airplay::ffi::sdl3::init::{SDL_INIT_VIDEO, SDL_Init, SDL_Quit};
use airplay::ffi::sdl3::keycode::{
    SDL_KMOD_CTRL, SDL_KMOD_NONE, SDL_Keycode, SDL_Keymod, SDLK_BACKSPACE, SDLK_V,
};
use airplay::ffi::sdl3::main::SDL_SetMainReady;
use airplay::ffi::sdl3::mouse::SDL_MOUSEWHEEL_NORMAL;
use egui::{Event, Key, MouseWheelUnit, pos2, vec2};

fn zeroed() -> SDL_Event {
    // SAFETY: SDL_Event ist eine reine Daten-Union; alle Nullen sind gültig.
    unsafe { std::mem::zeroed() }
}

fn text_event(text: &CString) -> SDL_Event {
    let mut e = zeroed();
    e.r#type = SDL_EVENT_TEXT_INPUT.0;
    e.text.text = text.as_ptr();
    e
}

fn key_event(key: SDL_Keycode, m: SDL_Keymod) -> SDL_Event {
    let mut e = zeroed();
    e.r#type = SDL_EVENT_KEY_DOWN.0;
    e.key.key = key;
    e.key.r#mod = m;
    e.key.down = true;
    e
}

fn input(screen: [u32; 2], ppp: f32, events: &[SDL_Event]) -> egui::RawInput {
    let mut inp = EguiInput::new();
    for e in events {
        inp.on_event(e);
    }
    inp.take_raw_input(screen, ppp, 1.0)
}

#[test]
fn sdl_text_input_becomes_egui_text() {
    let text = CString::new("Ä").unwrap();
    let raw = input([800, 600], 1.0, &[text_event(&text)]);
    assert!(
        raw.events.contains(&Event::Text("Ä".to_string())),
        "{:?}",
        raw.events
    );
}

#[test]
fn backspace_maps_to_egui_key() {
    let raw = input([800, 600], 1.0, &[key_event(SDLK_BACKSPACE, SDL_KMOD_NONE)]);
    assert!(
        raw.events.iter().any(|e| matches!(
            e,
            Event::Key {
                key: Key::Backspace,
                pressed: true,
                repeat: false,
                ..
            }
        )),
        "{:?}",
        raw.events
    );
}

#[test]
fn mouse_motion_scales_by_ppp() {
    let mut e = zeroed();
    e.r#type = SDL_EVENT_MOUSE_MOTION.0;
    e.motion.x = 200.0;
    e.motion.y = 50.0;
    let raw = input([800, 600], 2.0, &[e]);
    assert!(
        raw.events.contains(&Event::PointerMoved(pos2(100.0, 25.0))),
        "{:?}",
        raw.events
    );
    // Bildschirm in Punkten, Skalierung im Viewport
    let rect = raw.screen_rect.expect("screen_rect");
    assert_eq!(rect.size(), vec2(400.0, 300.0));
    assert_eq!(raw.viewport().native_pixels_per_point, Some(2.0));
}

#[test]
fn wheel_becomes_scroll() {
    let mut e = zeroed();
    e.r#type = SDL_EVENT_MOUSE_WHEEL.0;
    e.wheel.y = 1.0;
    e.wheel.direction = SDL_MOUSEWHEEL_NORMAL;
    let raw = input([800, 600], 1.0, &[e]);
    let wheel = raw.events.iter().find_map(|e| match e {
        Event::MouseWheel { unit, delta, .. } => Some((*unit, *delta)),
        _ => None,
    });
    assert_eq!(wheel, Some((MouseWheelUnit::Line, vec2(0.0, 1.0))));
}

/// SDL_Init/SDL_Quit sind prozessweit.
static SDL_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn ctrl_v_pastes_clipboard_text() {
    let _guard = SDL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let text = CString::new("Grüße Ä").unwrap();
    // SAFETY: SDL nur unter SDL_LOCK; SDL_Quit am Ende.
    unsafe {
        SDL_SetMainReady();
        assert!(SDL_Init(SDL_INIT_VIDEO), "SDL_Init");
        assert!(SDL_SetClipboardText(text.as_ptr()), "SDL_SetClipboardText");
    }
    let raw = input([800, 600], 1.0, &[key_event(SDLK_V, SDL_KMOD_CTRL)]);
    // SAFETY: passend zu SDL_Init oben.
    unsafe { SDL_Quit() };
    assert!(
        raw.events.contains(&Event::Paste("Grüße Ä".to_string())),
        "{:?}",
        raw.events
    );
}
