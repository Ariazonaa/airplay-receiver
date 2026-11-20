//! Port von test_hotkeys.cpp.
use airplay::app::hotkeys::{Command, map_key};
use airplay::ffi::sdl3::keycode::{
    SDL_KMOD_ALT, SDL_KMOD_LCTRL, SDL_KMOD_NONE, SDL_KMOD_RCTRL, SDL_KMOD_SHIFT, SDLK_A,
    SDLK_ESCAPE, SDLK_F9, SDLK_F10, SDLK_F11, SDLK_F12, SDLK_T,
};

#[test]
fn f11_toggles_fullscreen() {
    assert_eq!(
        map_key(SDLK_F11, SDL_KMOD_NONE, false),
        Some(Command::ToggleFullscreen)
    );
    assert_eq!(
        map_key(SDLK_F11, SDL_KMOD_NONE, true),
        Some(Command::ToggleFullscreen)
    );
}

#[test]
fn esc_only_leaves_fullscreen() {
    assert_eq!(
        map_key(SDLK_ESCAPE, SDL_KMOD_NONE, true),
        Some(Command::ExitFullscreen)
    );
    assert_eq!(map_key(SDLK_ESCAPE, SDL_KMOD_NONE, false), None);
}

#[test]
fn ctrl_t_toggles_always_on_top() {
    assert_eq!(
        map_key(SDLK_T, SDL_KMOD_LCTRL, false),
        Some(Command::ToggleOnTop)
    );
    assert_eq!(
        map_key(SDLK_T, SDL_KMOD_RCTRL, false),
        Some(Command::ToggleOnTop)
    );
    assert_eq!(map_key(SDLK_T, SDL_KMOD_NONE, false), None);
    assert_eq!(map_key(SDLK_T, SDL_KMOD_SHIFT, false), None);
}

#[test]
fn f9_screenshot_f10_recording() {
    assert_eq!(
        map_key(SDLK_F9, SDL_KMOD_NONE, false),
        Some(Command::Screenshot)
    );
    assert_eq!(
        map_key(SDLK_F10, SDL_KMOD_NONE, true),
        Some(Command::ToggleRecording)
    );
}

#[test]
fn other_keys_do_nothing() {
    assert_eq!(map_key(SDLK_A, SDL_KMOD_NONE, false), None);
    assert_eq!(map_key(SDLK_F12, SDL_KMOD_NONE, false), None);
}

/// Zusatz: Strg+Umschalt/Alt+T löst nichts aus (v1: andere Modifikatoren schließen aus).
#[test]
fn ctrl_with_other_modifiers_t_does_nothing() {
    assert_eq!(
        map_key(SDLK_T, SDL_KMOD_LCTRL | SDL_KMOD_SHIFT, false),
        None
    );
    assert_eq!(map_key(SDLK_T, SDL_KMOD_LCTRL | SDL_KMOD_ALT, false), None);
}
