//! Port von test_letterbox.cpp, dazu der Renderer bei Fenstergröße 0 (Review-Fokus 3) und der
//! Software-Upload (sws_scale nach NV12).

#[path = "support/ci.rs"]
mod ci;
#[path = "support/video_fixture.rs"]
mod video_fixture;

use std::ffi::CStr;
use std::sync::Mutex;

use airplay::ffi::sdl3::init::{SDL_INIT_VIDEO, SDL_Init, SDL_Quit};
use airplay::ffi::sdl3::main::SDL_SetMainReady;
use airplay::ffi::sdl3::video::{
    SDL_CreateWindow, SDL_DestroyWindow, SDL_WINDOW_HIDDEN, SDL_Window,
};
use airplay::media::video_decoder::VideoDecoder;
use airplay::render::letterbox::{Rect, letterbox};
use airplay::render::renderer::Renderer;
use video_fixture::{fresh_temp_dir, make_h264_fixture, split_access_units_default};

const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn portrait_iphone_in_16_9_window() {
    assert_eq!(letterbox(1170, 2532, 1920, 1080), rect(710, 0, 499, 1080));
}

#[test]
fn landscape_iphone_in_16_9_window() {
    assert_eq!(letterbox(2532, 1170, 1920, 1080), rect(0, 96, 1920, 887));
}

#[test]
fn same_aspect_fills_window() {
    assert_eq!(letterbox(1920, 1080, 1280, 720), rect(0, 0, 1280, 720));
}

#[test]
fn degenerate_sizes_do_not_divide_by_zero() {
    assert_eq!(letterbox(1920, 1080, 0, 0), rect(0, 0, 0, 0));
    assert_eq!(letterbox(0, 0, 1280, 720), rect(0, 0, 0, 0));
}

/// `Renderer::init` verlangt ein D3D11-Hardwaregerät mit Video-Unterstützung (kein WARP-Ersatz)
/// und eine Swapchain am Fenster; auf einem CI-Runner ohne GPU ist das nicht sicher.
fn skip_renderer_on_ci(test: &str) -> bool {
    ci::skip_on_ci(
        test,
        "GPU für Renderer::init (D3D11-Hardwaregerät + Swapchain)",
    )
}

/// SDL-Tests laufen nacheinander (SDL_Init/SDL_Quit sind prozessweit).
static SDL_LOCK: Mutex<()> = Mutex::new(());

/// Verstecktes SDL-Fenster für die Dauer von `f`.
fn with_hidden_window(f: impl FnOnce(*mut SDL_Window)) {
    let _guard = SDL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: SDL wird nur unter SDL_LOCK benutzt; Fenster und SDL werden am Ende freigegeben.
    unsafe {
        SDL_SetMainReady();
        assert!(SDL_Init(SDL_INIT_VIDEO), "SDL_Init fehlgeschlagen");
        let window = SDL_CreateWindow(c"renderer-test".as_ptr(), 320, 240, SDL_WINDOW_HIDDEN);
        assert!(!window.is_null(), "SDL_CreateWindow: {:?}", sdl_error());
        f(window);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
}

#[test]
fn renderer_resize_to_zero_is_ignored() {
    if skip_renderer_on_ci("renderer_resize_to_zero_is_ignored") {
        return;
    }
    with_hidden_window(|window| {
        let mut r = Renderer::init(window).expect("Renderer::init");
        let initial = (r.width(), r.height());
        assert!(initial.0 > 0 && initial.1 > 0);
        r.resize(0, 0);
        assert_eq!((r.width(), r.height()), initial);
        r.resize(0, 100);
        r.resize(100, 0);
        assert_eq!((r.width(), r.height()), initial);
        r.begin_frame();
        assert!(r.end_frame());
        r.resize(200, 100);
        assert_eq!((r.width(), r.height()), (200, 100));
        r.begin_frame();
        assert!(r.end_frame());
        assert!(!r.has_video());
    });
}

#[test]
fn upload_software_frame_draws() {
    if skip_renderer_on_ci("upload_software_frame_draws") {
        return;
    }
    let dir = fresh_temp_dir("render");
    assert!(make_h264_fixture(&dir.join("a.h264"), 640, 360, 3));
    let packets = split_access_units_default(&dir.join("a.h264"));
    let mut dec = VideoDecoder::default();
    assert!(dec.open(None, false));
    let frames: Vec<_> = packets.iter().flat_map(|p| dec.decode(p)).collect();
    assert_eq!(frames.len(), 3);
    assert!(frames.iter().all(|f| !f.is_hw()));

    with_hidden_window(|window| {
        let mut r = Renderer::init(window).expect("Renderer::init");
        for f in &frames {
            assert!(r.upload_frame(f));
            assert!(r.has_video());
            assert_eq!((r.video_width(), r.video_height()), (640, 360));
            r.begin_frame();
            r.draw_video(letterbox(
                r.video_width(),
                r.video_height(),
                r.width(),
                r.height(),
            ));
            assert!(r.end_frame());
        }
        r.clear_video();
        assert!(!r.has_video());
    });
}

fn sdl_error() -> String {
    let e = airplay::ffi::sdl3::error::SDL_GetError();
    if e.is_null() {
        return String::new();
    }
    // SAFETY: SDL liefert einen nullterminierten String.
    unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned()
}
