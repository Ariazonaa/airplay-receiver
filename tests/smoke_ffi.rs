//! Rauchtest: FFmpeg, SDL3 und der UxPlay-Kern lassen sich linken und aufrufen.

use airplay::ffi::{ffmpeg, sdl3, uxplay};

#[test]
fn ffmpeg_and_sdl_link() {
    // SAFETY: reine Versionsabfrage ohne Vorbedingungen.
    let avcodec = unsafe { ffmpeg::avcodec_version() };
    assert!(avcodec >> 16 >= 62, "libavcodec zu alt: {avcodec:#x}");

    let sdl = sdl3::version::SDL_GetVersion();
    assert!(sdl >= 3_004_000, "SDL zu alt: {sdl}");

    // SAFETY: liest nur die Systemuhr (CLOCK_REALTIME in ns).
    let now = unsafe { uxplay::get_local_time() };
    assert!(now > 0);
}
