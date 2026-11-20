//! Port von test_audio.cpp.

#[macro_use]
#[path = "support/approx.rs"]
mod approx;
#[allow(dead_code)] // hier nur fresh_temp_dir
#[path = "support/video_fixture.rs"]
mod video_fixture;

use std::process::Command;

use airplay::ffi::ffmpeg::AV_CODEC_ID_AAC;
use airplay::media::audio_decoder::{self, AudioDecoder};
use airplay::media::audio_output::SampleRing;
use airplay::protocol::packets::AudioPacket;
use video_fixture::fresh_temp_dir;

#[test]
fn volume_to_gain() {
    assert_eq!(audio_decoder::volume_to_gain(-144.0), 0.0);
    assert_eq!(audio_decoder::volume_to_gain(-200.0), 0.0);
    assert_approx!(audio_decoder::volume_to_gain(0.0), 1.0f32);
    assert_approx!(audio_decoder::volume_to_gain(-20.0), 0.1f32, 0.001);
    assert_approx!(audio_decoder::volume_to_gain(6.0), 1.0f32);
}

#[test]
fn open_supports_aac_eld_and_aac_lc_only() {
    let mut d = AudioDecoder::default();
    assert!(d.open(8));
    assert!(d.open(4));
    assert!(!d.open(2)); // ALAC: reiner Audio-Empfang ist kein Ziel
    assert!(!d.open(0));
}

#[test]
fn aac_decodes_to_44_1_khz_stereo_float() {
    let dir = fresh_temp_dir("audio");
    let file = dir.join("a.aac");
    let status = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:r=44100",
            "-t",
            "1",
            "-ac",
            "2",
            "-c:a",
            "aac",
            "-f",
            "adts",
        ])
        .arg(&file)
        .status()
        .expect("ffmpeg starten");
    assert!(status.success());
    let d = std::fs::read(&file).unwrap();

    let mut dec = AudioDecoder::default();
    assert!(dec.open_for_test(AV_CODEC_ID_AAC, Vec::new()));
    let mut samples = 0usize;
    let mut pos = 0usize;
    while pos + 7 <= d.len() {
        // ADTS-Rahmenlänge
        let len = (((d[pos + 3] & 0x03) as usize) << 11)
            | ((d[pos + 4] as usize) << 3)
            | ((d[pos + 5] as usize) >> 5);
        assert!(len > 7);
        let p = AudioPacket {
            data: d[pos..pos + len].to_vec(),
            ..Default::default()
        };
        samples += dec.decode(&p).len();
        pos += len;
    }
    assert_eq!(dec.sample_rate(), 44100);
    assert_eq!(dec.channels(), 2);
    let frames = samples / 2;
    assert!(frames > 40000, "{frames}");
    assert!(frames < 47000, "{frames}");
}

#[test]
fn sample_ring_fifo_drop_oldest_partial_pop() {
    let mut r = SampleRing::default();
    let a = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]; // 3 Stereo-Frames
    r.push(&a);
    assert_eq!(r.frames(), 3);
    r.drop_oldest(1);
    assert_eq!(r.frames(), 2);
    let mut out = [0.0f32; 8];
    assert_eq!(r.pop(&mut out), 2);
    assert_eq!(out[0], 3.0);
    assert_eq!(out[3], 6.0);
    assert_eq!(r.frames(), 0);
    r.drop_oldest(10); // mehr als vorhanden: kein Absturz
    assert_eq!(r.frames(), 0);
}
