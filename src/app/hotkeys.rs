//! Tastenkürzel: F11 Vollbild, Esc (nur im Vollbild) verlassen, Strg+T Immer im Vordergrund,
//! F9 Screenshot, F10 Aufnahme.

use crate::ffi::sdl3::keycode::{
    SDL_KMOD_ALT, SDL_KMOD_CTRL, SDL_KMOD_GUI, SDL_KMOD_SHIFT, SDL_Keycode, SDL_Keymod,
    SDLK_ESCAPE, SDLK_F9, SDLK_F10, SDLK_F11, SDLK_T,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    ToggleFullscreen,
    ExitFullscreen,
    ToggleOnTop,
    Screenshot,
    ToggleRecording,
}

pub fn map_key(key: SDL_Keycode, mods: SDL_Keymod, fullscreen: bool) -> Option<Command> {
    let ctrl = (mods & SDL_KMOD_CTRL).0 != 0;
    let other_mods = (mods & (SDL_KMOD_SHIFT | SDL_KMOD_ALT | SDL_KMOD_GUI)).0 != 0;
    match key {
        SDLK_F11 => Some(Command::ToggleFullscreen),
        SDLK_ESCAPE if fullscreen => Some(Command::ExitFullscreen),
        SDLK_T if ctrl && !other_mods => Some(Command::ToggleOnTop),
        SDLK_F9 => Some(Command::Screenshot),
        SDLK_F10 => Some(Command::ToggleRecording),
        _ => None,
    }
}

/// Wie `map_key`, aber ohne Kürzel, solange in einem egui-Textfeld getippt wird
/// (`text_edit_focused`, siehe `egui_input::egui_wants_text`). Ein nur per Tab fokussierter
/// Knopf schluckt die Kürzel nicht.
pub fn hotkey_command(
    key: SDL_Keycode,
    mods: SDL_Keymod,
    fullscreen: bool,
    text_edit_focused: bool,
) -> Option<Command> {
    if text_edit_focused {
        return None;
    }
    map_key(key, mods, fullscreen)
}
