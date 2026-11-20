//! Anwendungslogik: Log, Einstellungen, Texte, Sitzung, Tastenkürzel, Fenster, App.

#[allow(clippy::module_inception)]
pub mod app;
pub mod config;
pub mod demo;
pub mod egui_input;
pub mod hotkeys;
pub mod log;
pub mod session_controller;
pub mod settings_apply;
pub mod strings;
pub mod ui;
pub mod window;
