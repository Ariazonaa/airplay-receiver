//! SDL3-Ereignisse → egui-Eingabe (`RawInput`): Maus, Rad, Tasten, Texteingabe (UTF-8 aus
//! `SDL_EVENT_TEXT_INPUT`, damit Umlaute ankommen) und Zwischenablage über SDL.
//!
//! SDL liefert Mauskoordinaten auf Windows in Pixeln; egui rechnet in Punkten
//! (Pixel / `ppp`, `ppp` = `SDL_GetWindowDisplayScale`). Die Umrechnung passiert in
//! `take_raw_input`, weil erst dann die aktuelle Skalierung feststeht.

use std::ffi::{CStr, CString};

use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Rect, TouchPhase,
    pos2, vec2,
};

use crate::ffi::sdl3::clipboard::{SDL_GetClipboardText, SDL_SetClipboardText};
use crate::ffi::sdl3::events::{
    SDL_EVENT_KEY_DOWN, SDL_EVENT_KEY_UP, SDL_EVENT_MOUSE_BUTTON_DOWN, SDL_EVENT_MOUSE_BUTTON_UP,
    SDL_EVENT_MOUSE_MOTION, SDL_EVENT_MOUSE_WHEEL, SDL_EVENT_TEXT_INPUT,
    SDL_EVENT_WINDOW_FOCUS_GAINED, SDL_EVENT_WINDOW_FOCUS_LOST, SDL_EVENT_WINDOW_MOUSE_LEAVE,
    SDL_Event,
};
use crate::ffi::sdl3::keyboard::{SDL_StartTextInput, SDL_StopTextInput};
use crate::ffi::sdl3::keycode::*;
use crate::ffi::sdl3::mouse::{
    SDL_BUTTON_LEFT, SDL_BUTTON_MIDDLE, SDL_BUTTON_RIGHT, SDL_BUTTON_X1, SDL_BUTTON_X2,
    SDL_MOUSEWHEEL_FLIPPED,
};
use crate::ffi::sdl3::stdinc::SDL_free;
use crate::ffi::sdl3::video::SDL_Window;

/// Ein Ereignis vor der Umrechnung: Mauspositionen noch in Pixeln.
enum Pending {
    Ready(Event),
    Moved(Pos2),
    Button {
        pos: Pos2,
        button: PointerButton,
        pressed: bool,
        modifiers: Modifiers,
    },
}

#[derive(Default)]
pub struct EguiInput {
    pending: Vec<Pending>,
    modifiers: Modifiers,
    focused: bool,
    text_input: bool,
    /// zuletzt gemeldet: ein egui-Textfeld hat den Fokus (`egui_wants_text`)
    wants_keyboard: bool,
}

/// Tippt der Nutzer gerade in egui? Steuert SDL-Texteingabe und das Unterdrücken der
/// Tastenkürzel. Nur ein fokussiertes Textfeld zählt: `egui_wants_keyboard_input` wäre auch
/// für einen per Tab fokussierten Knopf wahr und würde F9/F10/F11/Esc/Strg+T schlucken.
pub fn egui_wants_text(ctx: &egui::Context) -> bool {
    ctx.text_edit_focused()
}

fn modifiers(m: SDL_Keymod) -> Modifiers {
    let has = |mask: SDL_Keymod| m.0 & mask.0 != 0;
    let ctrl = has(SDL_KMOD_CTRL);
    Modifiers {
        alt: has(SDL_KMOD_ALT),
        ctrl,
        shift: has(SDL_KMOD_SHIFT),
        mac_cmd: false,
        // Windows: Befehlstaste = Strg
        command: ctrl,
    }
}

/// SDL-Tastencode → egui-Taste (nur was Textfelder und Bedienung brauchen).
fn map_key(k: SDL_Keycode) -> Option<Key> {
    let letters = [
        (SDLK_A, Key::A),
        (SDLK_B, Key::B),
        (SDLK_C, Key::C),
        (SDLK_D, Key::D),
        (SDLK_E, Key::E),
        (SDLK_F, Key::F),
        (SDLK_G, Key::G),
        (SDLK_H, Key::H),
        (SDLK_I, Key::I),
        (SDLK_J, Key::J),
        (SDLK_K, Key::K),
        (SDLK_L, Key::L),
        (SDLK_M, Key::M),
        (SDLK_N, Key::N),
        (SDLK_O, Key::O),
        (SDLK_P, Key::P),
        (SDLK_Q, Key::Q),
        (SDLK_R, Key::R),
        (SDLK_S, Key::S),
        (SDLK_T, Key::T),
        (SDLK_U, Key::U),
        (SDLK_V, Key::V),
        (SDLK_W, Key::W),
        (SDLK_X, Key::X),
        (SDLK_Y, Key::Y),
        (SDLK_Z, Key::Z),
    ];
    if let Some((_, key)) = letters.iter().find(|(c, _)| *c == k) {
        return Some(*key);
    }
    Some(match k {
        SDLK_BACKSPACE => Key::Backspace,
        SDLK_DELETE => Key::Delete,
        SDLK_RETURN | SDLK_KP_ENTER => Key::Enter,
        SDLK_TAB => Key::Tab,
        SDLK_ESCAPE => Key::Escape,
        SDLK_SPACE => Key::Space,
        SDLK_LEFT => Key::ArrowLeft,
        SDLK_RIGHT => Key::ArrowRight,
        SDLK_UP => Key::ArrowUp,
        SDLK_DOWN => Key::ArrowDown,
        SDLK_HOME => Key::Home,
        SDLK_END => Key::End,
        SDLK_PAGEUP => Key::PageUp,
        SDLK_PAGEDOWN => Key::PageDown,
        SDLK_INSERT => Key::Insert,
        _ => return None,
    })
}

fn map_button(b: u8) -> Option<PointerButton> {
    Some(match i32::from(b) {
        SDL_BUTTON_LEFT => PointerButton::Primary,
        SDL_BUTTON_RIGHT => PointerButton::Secondary,
        SDL_BUTTON_MIDDLE => PointerButton::Middle,
        SDL_BUTTON_X1 => PointerButton::Extra1,
        SDL_BUTTON_X2 => PointerButton::Extra2,
        _ => return None,
    })
}

/// Text aus der SDL-Zwischenablage (leer → `None`).
fn clipboard_text() -> Option<String> {
    // SAFETY: SDL liefert einen nullterminierten String (oder null), den wir mit SDL_free freigeben.
    unsafe {
        let p = SDL_GetClipboardText();
        if p.is_null() {
            return None;
        }
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        SDL_free(p.cast());
        (!s.is_empty()).then_some(s)
    }
}

fn set_clipboard_text(text: &str) {
    if let Ok(c) = CString::new(text) {
        // SAFETY: nullterminierter String, SDL kopiert ihn.
        unsafe { SDL_SetClipboardText(c.as_ptr()) };
    }
}

impl EguiInput {
    pub fn new() -> Self {
        Self {
            focused: true,
            ..Default::default()
        }
    }

    /// Ereignis übernehmen. true = Tastatur-/Texteingabe, während ein egui-Textfeld den Fokus
    /// hat (die App soll sie nicht als Tastenkürzel auswerten).
    pub fn on_event(&mut self, e: &SDL_Event) -> bool {
        // SAFETY: Der Typ steht in jedem Ereignis; die Unterstrukturen nur beim passenden Typ.
        let ty = unsafe { e.r#type };
        if ty == SDL_EVENT_MOUSE_MOTION.0 {
            // SAFETY: Mausereignis.
            let m = unsafe { e.motion };
            self.pending.push(Pending::Moved(pos2(m.x, m.y)));
        } else if ty == SDL_EVENT_MOUSE_BUTTON_DOWN.0 || ty == SDL_EVENT_MOUSE_BUTTON_UP.0 {
            // SAFETY: Maustastenereignis.
            let b = unsafe { e.button };
            if let Some(button) = map_button(b.button) {
                self.pending.push(Pending::Moved(pos2(b.x, b.y)));
                self.pending.push(Pending::Button {
                    pos: pos2(b.x, b.y),
                    button,
                    pressed: b.down,
                    modifiers: self.modifiers,
                });
            }
        } else if ty == SDL_EVENT_MOUSE_WHEEL.0 {
            // SAFETY: Mausradereignis.
            let w = unsafe { e.wheel };
            let sign = if w.direction == SDL_MOUSEWHEEL_FLIPPED {
                -1.0
            } else {
                1.0
            };
            // SDL: y > 0 = vom Nutzer weg (Inhalt nach unten), x > 0 = nach rechts (Inhalt
            // nach links); egui: positive Werte schieben den Inhalt nach rechts/unten.
            self.pending.push(Pending::Ready(Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta: vec2(-w.x * sign, w.y * sign),
                phase: TouchPhase::Move,
                source: MouseWheelSource::Wheel,
                modifiers: self.modifiers,
            }));
        } else if ty == SDL_EVENT_KEY_DOWN.0 || ty == SDL_EVENT_KEY_UP.0 {
            // SAFETY: Tastaturereignis.
            let k = unsafe { e.key };
            self.modifiers = modifiers(k.r#mod);
            self.pending
                .push(Pending::Ready(Event::ModifiersChanged(self.modifiers)));
            let Some(key) = map_key(k.key) else {
                return self.wants_keyboard;
            };
            if k.down && self.modifiers.command {
                // Zwischenablage wie egui-winit: Strg+C/X/V werden zu Copy/Cut/Paste.
                match key {
                    Key::C => self.pending.push(Pending::Ready(Event::Copy)),
                    Key::X => self.pending.push(Pending::Ready(Event::Cut)),
                    Key::V => {
                        if let Some(text) = clipboard_text() {
                            self.pending.push(Pending::Ready(Event::Paste(text)));
                        }
                    }
                    _ => {}
                }
            }
            self.pending.push(Pending::Ready(Event::Key {
                key,
                physical_key: None,
                pressed: k.down,
                repeat: k.repeat,
                modifiers: self.modifiers,
            }));
            return self.wants_keyboard;
        } else if ty == SDL_EVENT_TEXT_INPUT.0 {
            // SAFETY: Textereignis; text ist nullterminiertes UTF-8 (oder null).
            let p = unsafe { e.text.text };
            if !p.is_null() {
                // SAFETY: siehe oben; SDL hält den Text bis zum nächsten Ereignis.
                let text = unsafe { CStr::from_ptr(p) }.to_string_lossy();
                let text: String = text.chars().filter(|c| !c.is_control()).collect();
                if !text.is_empty() {
                    self.pending.push(Pending::Ready(Event::Text(text)));
                }
            }
            return self.wants_keyboard;
        } else if ty == SDL_EVENT_WINDOW_MOUSE_LEAVE.0 {
            self.pending.push(Pending::Ready(Event::PointerGone));
        } else if ty == SDL_EVENT_WINDOW_FOCUS_GAINED.0 || ty == SDL_EVENT_WINDOW_FOCUS_LOST.0 {
            self.focused = ty == SDL_EVENT_WINDOW_FOCUS_GAINED.0;
            self.pending
                .push(Pending::Ready(Event::WindowFocused(self.focused)));
        }
        false
    }

    /// Gesammelte Ereignisse für den nächsten egui-Durchlauf; `screen_px` = Zielgröße in Pixeln.
    pub fn take_raw_input(&mut self, screen_px: [u32; 2], ppp: f32, time_s: f64) -> egui::RawInput {
        let ppp = if ppp.is_finite() && ppp > 0.0 {
            ppp
        } else {
            1.0
        };
        let to_points = |p: Pos2| pos2(p.x / ppp, p.y / ppp);
        let events = self
            .pending
            .drain(..)
            .map(|p| match p {
                Pending::Ready(e) => e,
                Pending::Moved(pos) => Event::PointerMoved(to_points(pos)),
                Pending::Button {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } => Event::PointerButton {
                    pos: to_points(pos),
                    button,
                    pressed,
                    modifiers,
                },
            })
            .collect();
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                vec2(screen_px[0] as f32 / ppp, screen_px[1] as f32 / ppp),
            )),
            time: Some(time_s),
            events,
            focused: self.focused,
            ..Default::default()
        };
        raw.viewports
            .entry(raw.viewport_id)
            .or_default()
            .native_pixels_per_point = Some(ppp);
        raw
    }

    /// SDL-Texteingabe (und IME) nur, solange ein egui-Textfeld den Fokus hat.
    #[expect(clippy::not_unsafe_ptr_arg_deref)] // SDL prüft den Fensterzeiger selbst
    pub fn set_text_input(&mut self, window: *mut SDL_Window, wants_keyboard: bool) {
        self.wants_keyboard = wants_keyboard;
        if wants_keyboard == self.text_input {
            return;
        }
        self.text_input = wants_keyboard;
        // SAFETY: SDL prüft den Fensterzeiger; Aufruf auf dem Haupt-Thread.
        unsafe {
            if wants_keyboard {
                SDL_StartTextInput(window);
            } else {
                SDL_StopTextInput(window);
            }
        }
    }

    /// Ausgaben von egui umsetzen (Kopieren/Ausschneiden → SDL-Zwischenablage).
    pub fn handle_output(&mut self, out: &egui::PlatformOutput) {
        for c in &out.commands {
            if let egui::OutputCommand::CopyText(text) = c
                && !text.is_empty()
            {
                set_clipboard_text(text);
            }
        }
    }
}
