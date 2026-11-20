//! Port von test_filenames.cpp und test_recorder.cpp, dazu `records_into_umlaut_dir` und
//! `drop_while_recording_finalizes_file`. Ergebnisse werden mit `ffprobe` (MSYS2) geprüft.

#[path = "support/video_fixture.rs"]
mod video_fixture;

use std::path::Path;
use std::process::Command;

use airplay::media::video_decoder::VideoDecoder;
use airplay::protocol::packets::VideoPacket;
use airplay::record::filenames::{self, LocalTime};
use airplay::record::recorder::{Recorder, VideoFormat};
use airplay::record::screenshot::save_screenshot;
use video_fixture::{
    fresh_temp_dir, make_h264_fixture, split_access_units, split_access_units_default,
};

// ------------------------------------------------------------------ Dateinamen

fn sample_time() -> LocalTime {
    LocalTime {
        year: 2026,
        month: 11,
        day: 19,
        hour: 14,
        minute: 3,
        second: 22,
    }
}

#[test]
fn timestamp_name() {
    assert_eq!(
        filenames::timestamp_name("AirPlay", &sample_time(), "png"),
        "AirPlay_2026-11-19_14-03-22.png"
    );
    assert_eq!(
        filenames::timestamp_name("AirPlay", &sample_time(), ""),
        "AirPlay_2026-11-19_14-03-22"
    );
}

#[test]
fn unique_path_appends_counter() {
    let dir = fresh_temp_dir("names");
    assert_eq!(filenames::unique_path(&dir, "a.png"), dir.join("a.png"));
    std::fs::write(dir.join("a.png"), "x").unwrap();
    assert_eq!(filenames::unique_path(&dir, "a.png"), dir.join("a_2.png"));
    std::fs::write(dir.join("a_2.png"), "x").unwrap();
    assert_eq!(filenames::unique_path(&dir, "a.png"), dir.join("a_3.png"));
}

#[test]
fn part_file_names() {
    assert_eq!(
        filenames::part_file_name("AirPlay_2026-11-19_14-03-22", 1, "mp4"),
        "AirPlay_2026-11-19_14-03-22.mp4"
    );
    assert_eq!(
        filenames::part_file_name("AirPlay_2026-11-19_14-03-22", 2, "mp4"),
        "AirPlay_2026-11-19_14-03-22_teil2.mp4"
    );
    assert_eq!(
        filenames::part_file_name("AirPlay_2026-11-19_14-03-22", 3, "mp4"),
        "AirPlay_2026-11-19_14-03-22_teil3.mp4"
    );
}

#[test]
fn keyframe_detection_h264() {
    let dir = fresh_temp_dir("names");
    assert!(make_h264_fixture(&dir.join("a.h264"), 320, 240, 5));
    let p = split_access_units_default(&dir.join("a.h264"));
    assert_eq!(p.len(), 5);
    assert!(filenames::contains_keyframe(&p[0]));
    assert!(!filenames::contains_keyframe(&p[1]));
}

#[test]
fn keyframe_detection_h265_nal_types() {
    let idr = VideoPacket {
        h265: true,
        // VPS + IDR_W_RADL(19)
        data: vec![0, 0, 0, 1, 0x40, 0x01, 0xAA, 0, 0, 0, 1, 0x26, 0x01, 0xBB],
        ..Default::default()
    };
    assert!(filenames::contains_keyframe(&idr));
    let trail = VideoPacket {
        h265: true,
        data: vec![0, 0, 0, 1, 0x02, 0x01, 0xCC], // TRAIL_R(1)
        ..Default::default()
    };
    assert!(!filenames::contains_keyframe(&trail));
}

// ------------------------------------------------------------------ Recorder

/// beliebige Startzeit 5 s
const START: u64 = 5_000_000_000;
const FRAME_NS: u64 = 33_333_333;

#[derive(Debug, Default)]
struct Probe {
    streams: usize,
    has_h264: bool,
    has_aac: bool,
    has_png: bool,
    seconds: f64,
    /// Ende des Tonstroms (Start + Dauer)
    audio_end_seconds: f64,
    width: i64,
    height: i64,
}

fn num(v: &serde_json::Value) -> f64 {
    match v {
        serde_json::Value::String(s) => s.parse().unwrap_or(0.0),
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Wie v1 `probe` (avformat), hier über `ffprobe -of json`.
fn probe(file: &Path) -> Probe {
    let mut p = Probe::default();
    let Ok(out) = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(file)
        .output()
    else {
        return p;
    };
    if !out.status.success() {
        return p;
    }
    let Ok(j) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return p;
    };
    let streams = j["streams"].as_array().cloned().unwrap_or_default();
    p.streams = streams.len();
    for st in &streams {
        match st["codec_name"].as_str().unwrap_or("") {
            "h264" => {
                p.has_h264 = true;
                p.width = st["width"].as_i64().unwrap_or(0);
                p.height = st["height"].as_i64().unwrap_or(0);
            }
            "aac" => {
                p.has_aac = true;
                p.audio_end_seconds = num(&st["start_time"]) + num(&st["duration"]);
            }
            "png" => {
                p.has_png = true;
                p.width = st["width"].as_i64().unwrap_or(0);
                p.height = st["height"].as_i64().unwrap_or(0);
            }
            _ => {}
        }
    }
    p.seconds = num(&j["format"]["duration"]);
    p
}

fn feed_audio(r: &Recorder, start_ns: u64, seconds: f64) {
    const CHUNK: usize = 480;
    let mut buf = vec![0f32; CHUNK * 2];
    let total = (seconds * 44100.0) as usize;
    let mut pos = 0;
    while pos < total {
        for i in 0..CHUNK {
            let v = 0.2
                * (2.0 * std::f64::consts::PI * 440.0 * (pos + i) as f64 / 44100.0).sin() as f32;
            buf[i * 2] = v;
            buf[i * 2 + 1] = v;
        }
        r.push_audio(&buf, start_ns + (pos as f64 * 1e9 / 44100.0) as u64);
        pos += CHUNK;
    }
}

fn fmt(width: i32, height: i32) -> VideoFormat {
    VideoFormat {
        width,
        height,
        h265: false,
    }
}

#[test]
fn records_playable_mp4() {
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 640, 360, 60));
    let packets = split_access_units(&dir.join("a.h264"), START, FRAME_NS);
    let r = Recorder::default();
    assert!(r.start(&dir.join("out"), fmt(640, 360)));
    assert!(r.active());
    for p in &packets {
        r.push_video(p);
    }
    feed_audio(&r, START, 2.0);
    r.stop();
    assert!(!r.active());
    let files = r.files();
    assert_eq!(files.len(), 1);
    let pr = probe(&files[0]);
    assert_eq!(pr.streams, 2, "{pr:?}");
    assert!(pr.has_h264);
    assert!(pr.has_aac);
    assert!(pr.seconds > 1.8, "{pr:?}");
    assert!(pr.seconds < 2.2, "{pr:?}");
}

#[test]
fn resolution_change_starts_part_2() {
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 640, 360, 30));
    assert!(make_h264_fixture(&dir.join("b.h264"), 360, 640, 30));
    let a = split_access_units(&dir.join("a.h264"), START, FRAME_NS);
    let b = split_access_units(&dir.join("b.h264"), START + 30 * FRAME_NS, FRAME_NS);
    let r = Recorder::default();
    assert!(r.start(&dir.join("out"), fmt(640, 360)));
    for p in &a {
        r.push_video(p);
    }
    r.on_format_change(fmt(360, 640));
    for p in &b {
        r.push_video(p);
    }
    feed_audio(&r, START, 2.0);
    r.stop();
    let files = r.files();
    assert_eq!(files.len(), 2);
    let name1 = files[1].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name1.contains("_teil2.mp4"), "{name1}");
    assert_eq!(
        format!("{}_teil2", files[0].file_stem().unwrap().to_string_lossy()),
        files[1].file_stem().unwrap().to_string_lossy()
    );
    let p1 = probe(&files[0]);
    let p2 = probe(&files[1]);
    assert_eq!(p1.width, 640);
    assert_eq!(p2.width, 360);
    assert_eq!(p2.height, 640);
    assert!(p2.has_aac);
}

#[test]
fn start_mid_stream_uses_cached_keyframe() {
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 640, 360, 60));
    let packets = split_access_units(&dir.join("a.h264"), START, FRAME_NS);
    let r = Recorder::default();
    for p in &packets[..20] {
        r.push_video(p); // läuft schon, keine Aufnahme
    }
    assert!(r.start(&dir.join("out"), fmt(640, 360)));
    for p in &packets[20..] {
        r.push_video(p);
    }
    feed_audio(&r, START + 19 * FRAME_NS, (40 * FRAME_NS) as f64 / 1e9);
    r.stop();
    assert_eq!(r.files().len(), 1);
    let pr = probe(&r.files()[0]);
    assert!(pr.has_h264);
    assert!(pr.has_aac);
    // Wiedergabe beginnt beim Start der Aufnahme (Bild 20), nicht beim Keyframe (Bild 0):
    // 41 Bilder ≈ 1,37 s statt 2 s.
    println!("Dauer {} s", pr.seconds);
    assert!(pr.seconds > 1.2, "{pr:?}");
    assert!(pr.seconds < 1.55, "{pr:?}");
}

#[test]
fn keyframe_cache_has_no_time_limit() {
    // AirPlay schickt selten Keyframes: auch nach 2 Minuten muss F10 sofort aufnehmen können.
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 320, 240, 60));
    let packets = split_access_units(&dir.join("a.h264"), START, 2_000_000_000); // 2 s Abstand → 120 s
    let r = Recorder::default();
    for p in &packets {
        r.push_video(p);
    }
    assert!(r.start(&dir.join("out"), fmt(320, 240)));
    r.sync();
    assert!(!r.waiting_for_keyframe());
    assert_eq!(r.files().len(), 1);
    r.stop();
}

#[test]
fn after_cache_overflow_recording_waits_and_reports_it() {
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 640, 360, 30));
    assert!(make_h264_fixture(&dir.join("b.h264"), 640, 360, 10));
    let a = split_access_units(&dir.join("a.h264"), START, FRAME_NS);
    let b = split_access_units(&dir.join("b.h264"), START + 30 * FRAME_NS, FRAME_NS);
    let total: usize = a.iter().map(|p| p.data.len()).sum();
    let r = Recorder::new(total / 2); // Vorrat läuft über
    for p in &a {
        r.push_video(p);
    }
    assert!(r.start(&dir.join("out"), fmt(640, 360)));
    r.sync();
    assert!(r.waiting_for_keyframe());
    assert!(r.files().is_empty());
    for p in &b {
        r.push_video(p); // neuer Keyframe
    }
    r.sync();
    assert!(!r.waiting_for_keyframe());
    assert_eq!(r.files().len(), 1);
    r.stop();
}

#[test]
fn audio_gap_stays_a_gap_in_the_recording() {
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 320, 240, 90));
    let packets = split_access_units(&dir.join("a.h264"), START, FRAME_NS); // 3 s Video
    let r = Recorder::default();
    assert!(r.start(&dir.join("out"), fmt(320, 240)));
    for p in &packets {
        r.push_video(p);
    }
    feed_audio(&r, START, 1.0); // 0–1 s
    feed_audio(&r, START + 1_500_000_000, 1.0); // Lücke 1–1,5 s, dann 1,5–2,5 s
    r.stop();
    assert_eq!(r.files().len(), 1);
    let pr = probe(&r.files()[0]);
    println!("Ton endet bei {} s", pr.audio_end_seconds);
    assert!(pr.audio_end_seconds > 2.4, "{pr:?}"); // ohne Lückenbehandlung ~2,0 s
    assert!(pr.audio_end_seconds < 2.6, "{pr:?}");
}

#[test]
fn stop_is_idempotent() {
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 320, 240, 10));
    let packets = split_access_units(&dir.join("a.h264"), START, FRAME_NS);
    let r = Recorder::default();
    assert!(r.start(&dir.join("out"), fmt(320, 240)));
    for p in &packets {
        r.push_video(p);
    }
    r.stop();
    r.stop();
    assert_eq!(r.files().len(), 1);
    assert!(probe(&r.files()[0]).has_h264);
}

#[test]
fn screenshot_writes_png() {
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 640, 360, 1));
    let packets = split_access_units_default(&dir.join("a.h264"));
    let mut dec = VideoDecoder::default();
    assert!(dec.open(None, false));
    let frames = dec.decode(&packets[0]);
    assert_eq!(frames.len(), 1);
    let file = dir.join("shots").join("s.png");
    assert!(save_screenshot(&frames[0], &file));
    let pr = probe(&file);
    assert!(pr.has_png, "{pr:?}");
    assert_eq!(pr.width, 640);
    assert_eq!(pr.height, 360);
}

// ------------------------------------------------------------------ zusätzlich (Rust)

#[test]
fn records_into_umlaut_dir() {
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 320, 240, 30));
    let packets = split_access_units(&dir.join("a.h264"), START, FRAME_NS);
    let out = dir.join("Aufnahmen Ä");
    let r = Recorder::default();
    assert!(r.start(&out, fmt(320, 240)));
    for p in &packets {
        r.push_video(p);
    }
    feed_audio(&r, START, 1.0);
    r.stop();
    assert!(r.last_error().is_empty(), "{}", r.last_error());
    let files = r.files();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].parent(), Some(out.as_path()));
    assert!(files[0].exists());
    let pr = probe(&files[0]);
    assert!(pr.has_h264, "{pr:?}");
    assert!(pr.has_aac, "{pr:?}");
    assert!(pr.seconds > 0.0, "{pr:?}");
}

#[test]
fn drop_while_recording_finalizes_file() {
    let dir = fresh_temp_dir("rec");
    assert!(make_h264_fixture(&dir.join("a.h264"), 320, 240, 30));
    let packets = split_access_units(&dir.join("a.h264"), START, FRAME_NS);
    let file = {
        let r = Recorder::default();
        assert!(r.start(&dir.join("out"), fmt(320, 240)));
        for p in &packets {
            r.push_video(p);
        }
        feed_audio(&r, START, 1.0);
        r.sync();
        let file = r.current_file();
        assert!(!file.as_os_str().is_empty());
        file
        // r wird hier mit laufender Aufnahme gedroppt
    };
    let pr = probe(&file);
    assert!(pr.has_h264, "{pr:?}");
    assert!(pr.seconds > 0.0, "{pr:?}");
}
