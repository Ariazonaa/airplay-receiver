//! SDL-Fenster als RAII-Hülle (SDL_Init + Fenster, Drop: Fenster zerstören + SDL_Quit) und
//! kleine SDL-Helfer für die App.

use std::ffi::{CStr, CString};

use crate::app::log::LogLevel;
use crate::ffi::sdl3::error::SDL_GetError;
use crate::ffi::sdl3::events::{SDL_Event, SDL_PollEvent};
use crate::ffi::sdl3::hints::{
    SDL_HINT_WINDOWS_INTRESOURCE_ICON, SDL_HINT_WINDOWS_INTRESOURCE_ICON_SMALL, SDL_SetHint,
};
use crate::ffi::sdl3::init::{SDL_INIT_AUDIO, SDL_INIT_VIDEO, SDL_Init, SDL_Quit};
use crate::ffi::sdl3::timer::SDL_GetTicks;
use crate::ffi::sdl3::video::{
    SDL_CreateWindow, SDL_DestroyWindow, SDL_GetWindowDisplayScale, SDL_GetWindowFlags,
    SDL_GetWindowID, SDL_SetWindowAlwaysOnTop, SDL_SetWindowFullscreen, SDL_SetWindowMinimumSize,
    SDL_SetWindowTitle, SDL_WINDOW_HIGH_PIXEL_DENSITY, SDL_WINDOW_MINIMIZED, SDL_WINDOW_RESIZABLE,
    SDL_Window,
};
use crate::log_msg;

/// Letzte SDL-Fehlermeldung.
pub fn sdl_error() -> String {
    let e = SDL_GetError();
    if e.is_null() {
        return String::new();
    }
    // SAFETY: SDL liefert einen nullterminierten String.
    unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned()
}

/// Millisekunden seit SDL_Init (wie v1 `SDL_GetTicks`).
pub fn ticks() -> u64 {
    // SAFETY: keine Vorbedingungen.
    unsafe { SDL_GetTicks() }
}

/// Nächstes Ereignis aus der SDL-Warteschlange (kann Event-Watches auslösen).
pub fn poll_event() -> Option<SDL_Event> {
    // SAFETY: SDL_Event ist eine reine Daten-Union; SDL füllt sie bei true.
    unsafe {
        let mut e: SDL_Event = std::mem::zeroed();
        SDL_PollEvent(&mut e).then_some(e)
    }
}

/// Hauptfenster; nur auf dem Haupt-Thread benutzen.
pub struct Window {
    raw: *mut SDL_Window,
}

impl Window {
    /// SDL_Init (Video + Audio) und Fenster 1280×720, Mindestgröße 480×320. Bei Fehler ist SDL
    /// wieder beendet; der Fehler steht schon im Log.
    pub fn create(title: &str) -> Result<Window, String> {
        // SAFETY: SDL-Aufrufe auf dem Haupt-Thread nach SDL_SetMainReady (main.rs).
        unsafe {
            // Fenster- und Taskleisten-Icon aus der eingebetteten Ressource 1 (build.rs).
            SDL_SetHint(SDL_HINT_WINDOWS_INTRESOURCE_ICON, c"1".as_ptr());
            SDL_SetHint(SDL_HINT_WINDOWS_INTRESOURCE_ICON_SMALL, c"1".as_ptr());
            if !SDL_Init(SDL_INIT_VIDEO | SDL_INIT_AUDIO) {
                let msg = format!("SDL_Init: {}", sdl_error());
                log_msg!(LogLevel::Error, "{}", msg);
                SDL_Quit();
                return Err(msg);
            }
            let title = CString::new(title).unwrap_or_default();
            let raw = SDL_CreateWindow(
                title.as_ptr(),
                1280,
                720,
                SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY,
            );
            if raw.is_null() {
                let msg = format!("SDL_CreateWindow: {}", sdl_error());
                log_msg!(LogLevel::Error, "{}", msg);
                SDL_Quit();
                return Err(msg);
            }
            SDL_SetWindowMinimumSize(raw, 480, 320);
            Ok(Window { raw })
        }
    }

    pub fn raw(&self) -> *mut SDL_Window {
        self.raw
    }

    pub fn id(&self) -> u32 {
        // SAFETY: gültiges Fenster.
        unsafe { SDL_GetWindowID(self.raw) }.0
    }

    pub fn minimized(&self) -> bool {
        // SAFETY: gültiges Fenster.
        let flags = unsafe { SDL_GetWindowFlags(self.raw) };
        (flags & SDL_WINDOW_MINIMIZED) == SDL_WINDOW_MINIMIZED
    }

    /// Anzeigeskalierung (z. B. 1.5 bei 150 %); egui-Punkte = Pixel / Skalierung.
    pub fn display_scale(&self) -> f32 {
        // SAFETY: gültiges Fenster.
        let s = unsafe { SDL_GetWindowDisplayScale(self.raw) };
        if s.is_finite() && s > 0.0 { s } else { 1.0 }
    }

    pub fn set_title(&self, title: &str) {
        if let Ok(t) = CString::new(title) {
            // SAFETY: gültiges Fenster, nullterminierter Titel.
            unsafe { SDL_SetWindowTitle(self.raw, t.as_ptr()) };
        }
    }

    pub fn set_fullscreen(&self, on: bool) {
        // SAFETY: gültiges Fenster.
        unsafe { SDL_SetWindowFullscreen(self.raw, on) };
    }

    pub fn set_always_on_top(&self, on: bool) {
        // SAFETY: gültiges Fenster.
        unsafe { SDL_SetWindowAlwaysOnTop(self.raw, on) };
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: Fenster aus `create`, danach nicht mehr benutzt; SDL_Quit passt zu SDL_Init.
        unsafe {
            SDL_DestroyWindow(self.raw);
            SDL_Quit();
        }
    }
}
