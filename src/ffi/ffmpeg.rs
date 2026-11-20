//! Bindings an FFmpeg (avcodec, avformat, avutil, swscale, swresample).
//! D3D11-Schnittstellen aus `hwcontext_d3d11va.h` sind opake Typen.

#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::all
)]

include!(concat!(env!("OUT_DIR"), "/ffmpeg.rs"));
