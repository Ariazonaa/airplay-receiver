//! Port von test_audio_pipeline.cpp.

#[macro_use]
#[path = "support/approx.rs"]
mod approx;
#[allow(dead_code)] // hier nur fresh_temp_dir
#[path = "support/video_fixture.rs"]
mod video_fixture;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use airplay::app::config::Mode;
use airplay::media::audio_output::AudioSink;
use airplay::media::audio_pipeline::AudioPipeline;
use airplay::protocol::packet_queue::PacketQueue;
use airplay::protocol::packets::AudioPacket;
use video_fixture::fresh_temp_dir;

/// Monotone Uhr in ns (mit 1 s Versatz, damit `arrival_ns` nie 0 ist).
fn steady_ns() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1_000_000_000
}

#[derive(Default)]
struct FakeState {
    opened: bool,
    queued: f64,
    total: usize,
    first: u64,
    last: u64,
    starved_ns: u64,
}

impl FakeState {
    fn consume(&mut self) {
        let now = steady_ns();
        if self.last != 0 {
            let want = (now - self.last) as f64 * 44100.0 / 1e9;
            if self.opened && self.total > 0 && want > self.queued {
                let s = ((want - self.queued) * 1e9 / 44100.0) as u64;
                self.starved_ns += s;
                if std::env::var_os("PIPE_DEBUG").is_some() {
                    println!(
                        "leer {:.1} ms bei t={:.0} ms (ausgegeben {:.0} ms)",
                        s as f64 / 1e6,
                        (now - self.first) as f64 / 1e6,
                        self.total as f64 * 1000.0 / 44100.0
                    );
                }
            }
            self.queued = (self.queued - want).max(0.0);
        }
        if self.first == 0 {
            self.first = now;
        }
        self.last = now;
    }
}

/// Ausgabe-Attrappe: verbraucht in Echtzeit 44 100 Frames/s, zählt was ankommt und Leerlauf.
#[derive(Default)]
struct FakeSink {
    m: Mutex<FakeState>,
}

impl FakeSink {
    fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.m.lock().unwrap()
    }

    fn total(&self) -> usize {
        self.st().total
    }

    fn starved_ms(&self) -> f64 {
        self.st().starved_ns as f64 / 1e6
    }
}

impl AudioSink for FakeSink {
    fn open(&self, _rate: i32, _channels: i32) -> bool {
        self.st().opened = true;
        true
    }

    fn is_open(&self) -> bool {
        self.st().opened
    }

    fn push(&self, interleaved: &[f32]) {
        let frames = interleaved.len() / 2;
        let mut s = self.st();
        s.consume();
        s.queued += frames as f64;
        s.total += frames;
    }

    fn queued_frames(&self) -> usize {
        let mut s = self.st();
        s.consume();
        s.queued as usize
    }

    fn drop_oldest(&self, frames: usize) {
        let mut s = self.st();
        s.queued = (s.queued - frames as f64).max(0.0);
    }

    fn clear(&self) {
        self.st().queued = 0.0;
    }

    fn set_gain(&self, _gain: f32) {}

    fn set_rate_ratio(&self, _ratio: f32) {}
}

/// Rohe AAC-LC-Rahmen (ohne ADTS-Kopf) wie AirPlay sie mit ct=4 schickt.
fn aac_frames(dir: &Path, seconds: f64) -> Vec<Vec<u8>> {
    let file = dir.join("a.aac");
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg("sine=f=440:r=44100")
        .arg("-t")
        .arg(seconds.to_string())
        .args(["-ac", "2", "-c:a", "aac", "-f", "adts"])
        .arg(&file)
        .status()
        .expect("ffmpeg starten");
    assert!(status.success());
    let d = std::fs::read(&file).unwrap();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 7 <= d.len() {
        let len = (((d[pos + 3] & 0x03) as usize) << 11)
            | ((d[pos + 4] as usize) << 3)
            | ((d[pos + 5] as usize) >> 5);
        let hdr = if d[pos + 1] & 0x01 != 0 { 7 } else { 9 };
        out.push(d[pos + hdr..pos + len].to_vec());
        pos += len;
    }
    out
}

#[test]
fn audio_keeps_playing_while_the_window_thread_is_blocked() {
    let frames = aac_frames(&fresh_temp_dir("pipe"), 1.5);
    let queue = Arc::new(PacketQueue::<AudioPacket>::new(400));
    let sink = Arc::new(FakeSink::default());
    let pipe = AudioPipeline::new(queue.clone(), sink.clone(), Box::new(steady_ns));
    pipe.configure(Mode::LowLatency, 200);
    pipe.start();

    // Absender im eigenen Thread, Echtzeit-Takt (1024 Samples ≈ 23,2 ms).
    let q = queue.clone();
    let sender = thread::spawn(move || {
        let t0 = steady_ns();
        for (i, f) in frames.into_iter().enumerate() {
            let send_at = t0 + (i as f64 * 1024.0 * 1e9 / 44100.0) as u64;
            while steady_ns() < send_at {
                thread::sleep(Duration::from_millis(1));
            }
            let arrival = steady_ns();
            q.push(AudioPacket {
                data: f,
                ct: 4,
                arrival_ns: arrival,
                ntp_local: arrival + 183_000_000,
                ..Default::default()
            });
        }
    });
    // "Fenster-Thread" blockiert die ganze Zeit (wie beim Verschieben).
    thread::sleep(Duration::from_millis(1500));
    sender.join().unwrap();
    let starved_while_streaming = sink.starved_ms(); // danach ist der Strom zu Ende
    thread::sleep(Duration::from_millis(100));
    pipe.stop();

    let seconds = sink.total() as f64 / 44100.0;
    println!("ausgegeben {seconds} s, Leerlauf während des Stroms {starved_while_streaming} ms");
    assert!(seconds > 1.3, "{seconds}");
    assert!(starved_while_streaming < 30.0, "{starved_while_streaming}");
}

#[test]
fn lead_is_measured_from_arrival_time_not_processing_time() {
    let frames = aac_frames(&fresh_temp_dir("pipe"), 0.6);
    let queue = Arc::new(PacketQueue::<AudioPacket>::new(400));
    let sink = Arc::new(FakeSink::default());
    let fake_now: u64 = 10_000_000_000;
    let pipe = AudioPipeline::new(queue.clone(), sink, Box::new(move || fake_now));
    pipe.configure(Mode::Smooth, 200);
    for (i, f) in frames.into_iter().enumerate() {
        let arrival = 9_000_000_000u64 + i as u64 * 23_000_000;
        queue.push(AudioPacket {
            data: f,
            ct: 4,
            arrival_ns: arrival,
            ntp_local: arrival + 183_000_000,
            ..Default::default()
        });
    }
    pipe.step(); // Verarbeitung erst viel später (Hauptthread hing 1 s)
    assert_approx!(pipe.stats().audio_lead_ms, 183.0f32, 0.01);
    assert_eq!(pipe.present_offset_ns(), 17_000_000); // 200 ms gesamt − 183 ms Vorlauf
}
