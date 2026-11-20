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

/// Gelinkt ist das eigene, minimale FFmpeg (`scripts/build-ffmpeg.sh`) und nicht das
/// MSYS2-FFmpeg mit allen Codecs: genau die benutzten Komponenten sind vorhanden, sonst nichts.
#[test]
fn ffmpeg_is_custom_minimal_build() {
    use std::ffi::CStr;

    // SAFETY: liefert einen statischen, nullterminierten String.
    let config = unsafe { CStr::from_ptr(ffmpeg::avcodec_configuration()) }
        .to_string_lossy()
        .into_owned();
    assert!(
        config.contains("--disable-everything"),
        "kein minimaler FFmpeg-Build: {config}"
    );
    assert!(
        !config.contains("--enable-gpl"),
        "FFmpeg mit --enable-gpl gebaut: {config}"
    );

    // SAFETY: reine Nachschlagefunktionen ohne Vorbedingungen.
    unsafe {
        for (name, id) in [
            ("H264", ffmpeg::AV_CODEC_ID_H264),
            ("HEVC", ffmpeg::AV_CODEC_ID_HEVC),
            ("AAC", ffmpeg::AV_CODEC_ID_AAC),
        ] {
            assert!(
                !ffmpeg::avcodec_find_decoder(id).is_null(),
                "Dekoder {name} fehlt"
            );
        }
        for (name, id) in [
            ("AAC", ffmpeg::AV_CODEC_ID_AAC),
            ("PNG", ffmpeg::AV_CODEC_ID_PNG),
        ] {
            assert!(
                !ffmpeg::avcodec_find_encoder(id).is_null(),
                "Enkoder {name} fehlt"
            );
        }
        assert!(
            !ffmpeg::av_guess_format(c"mp4".as_ptr(), std::ptr::null(), std::ptr::null()).is_null(),
            "mp4-Muxer fehlt"
        );
        assert_eq!(
            ffmpeg::av_hwdevice_find_type_by_name(c"d3d11va".as_ptr()),
            ffmpeg::AV_HWDEVICE_TYPE_D3D11VA,
            "D3D11VA fehlt"
        );
        assert!(
            ffmpeg::avcodec_find_decoder(ffmpeg::AV_CODEC_ID_VP9).is_null(),
            "VP9-Dekoder vorhanden, also nicht der minimale Build"
        );
    }
}
