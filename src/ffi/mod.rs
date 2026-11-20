//! Rohe C-Bindings (bindgen). `unsafe` gehört nur in die sicheren Hüllen darüber.

pub mod ffmpeg;
pub mod uxplay;

/// SDL3 kommt fertig aus `sdl3-sys` (per pkg-config verlinkt).
pub use sdl3_sys as sdl3;
