//! airplay.exe: GUI-Programm (kein Konsolenfenster); Protokoll geht ins Log.
#![cfg_attr(not(test), windows_subsystem = "windows")]

use airplay::app::app::{App, log_startup_error};
use airplay::app::demo::parse_args;
use airplay::ffi::sdl3::main::SDL_SetMainReady;

fn main() -> Result<(), String> {
    // args_os statt args: ein nicht darstellbarer Pfad ergibt eine Fehlermeldung statt Panik.
    let args = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned());
    let options = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            log_startup_error(&e);
            std::process::exit(2);
        }
    };
    // SAFETY: Wir benutzen kein SDL_main; vor SDL_Init aufzurufen.
    unsafe { SDL_SetMainReady() };
    App::new_with(options)?.run();
    Ok(())
}
