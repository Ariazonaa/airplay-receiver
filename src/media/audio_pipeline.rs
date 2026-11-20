//! Ton-Verarbeitung im eigenen Thread: Pakete holen, dekodieren, planen, ausgeben.
//! Unabhängig vom Fenster-Thread, damit Fensterverschieben o. Ä. den Ton nicht anhält.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};

use crate::app::config::{Mode, SMOOTH_DELAY_DEFAULT_MS};
use crate::app::log::LogLevel;
use crate::log_msg;
use crate::media::audio_decoder::AudioDecoder;
use crate::media::audio_output::{AudioSink, SampleRing};
use crate::media::scheduler::{AudioScheduler, LeadEstimator, OffsetTracker};
use crate::protocol::packet_queue::PacketQueue;
use crate::protocol::packets::AudioPacket;

const RATE: i32 = 44100;
const DIAG_INTERVAL_NS: u64 = 2_000_000_000;
const STEP_PAUSE: Duration = Duration::from_millis(4);

fn frames_to_ms(frames: usize) -> f32 {
    frames as f32 * 1000.0 / RATE as f32
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioStats {
    /// Ausgabe gesamt
    pub buffer_ms: f32,
    /// wartet auf Freigabe
    pub pending_ms: f32,
    pub ratio: f32,
    /// gemessener Vorlauf des Senders
    pub audio_lead_ms: f32,
}

impl Default for AudioStats {
    fn default() -> Self {
        Self {
            buffer_ms: 0.0,
            pending_ms: 0.0,
            ratio: 1.0,
            audio_lead_ms: 0.0,
        }
    }
}

/// Bekommt jeden dekodierten Block (interleaved Stereo-Float, Zeitstempel in ns), z. B. Aufnahme.
pub type PcmTap = Box<dyn FnMut(&[f32], u64) + Send>;
pub type Clock = Box<dyn Fn() -> u64 + Send>;

/// Von außen gesetzt, im Thread übernommen.
struct Cfg {
    mode: Mode,
    delay_ms: u32,
    dirty: bool,
}

/// Nur im Ton-Thread (bzw. in `step`) benutzt.
struct Worker {
    clock: Clock,
    decoder: AudioDecoder,
    sched: AudioScheduler,
    pending: SampleRing,
    lead: LeadEstimator,
    offsets: OffsetTracker,
    active_mode: Mode,
    active_delay_ms: u32,
    // Diagnose
    last_diag_ns: u64,
    released: usize,
    discarded: usize,
    dropped: usize,
}

struct Shared {
    source: Arc<PacketQueue<AudioPacket>>,
    sink: Arc<dyn AudioSink>,
    running: AtomicBool,
    cfg: Mutex<Cfg>,
    reset_requested: AtomicBool,
    /// f32-Bits
    gain: AtomicU32,
    gain_dirty: AtomicBool,
    /// `i64::MIN` = unbekannt
    video_lead_ns: AtomicI64,
    tap: Mutex<Option<PcmTap>>,
    offset_ns: AtomicI64,
    stats: Mutex<AudioStats>,
    worker: Mutex<Worker>,
}

pub struct AudioPipeline {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl AudioPipeline {
    pub fn new(
        source: Arc<PacketQueue<AudioPacket>>,
        sink: Arc<dyn AudioSink>,
        clock: Clock,
    ) -> Self {
        let default_offset = i64::from(SMOOTH_DELAY_DEFAULT_MS) * 1_000_000;
        Self {
            shared: Arc::new(Shared {
                source,
                sink,
                running: AtomicBool::new(false),
                cfg: Mutex::new(Cfg {
                    mode: Mode::LowLatency,
                    delay_ms: SMOOTH_DELAY_DEFAULT_MS,
                    dirty: true,
                }),
                reset_requested: AtomicBool::new(false),
                gain: AtomicU32::new(1.0f32.to_bits()),
                gain_dirty: AtomicBool::new(true),
                video_lead_ns: AtomicI64::new(i64::MIN),
                tap: Mutex::new(None),
                offset_ns: AtomicI64::new(default_offset),
                stats: Mutex::new(AudioStats::default()),
                worker: Mutex::new(Worker {
                    clock,
                    decoder: AudioDecoder::default(),
                    sched: AudioScheduler::default(),
                    pending: SampleRing::default(),
                    lead: LeadEstimator::default(),
                    offsets: OffsetTracker::default(),
                    active_mode: Mode::LowLatency,
                    active_delay_ms: SMOOTH_DELAY_DEFAULT_MS,
                    last_diag_ns: 0,
                    released: 0,
                    discarded: 0,
                    dropped: 0,
                }),
            }),
            thread: Mutex::new(None),
        }
    }

    pub fn start(&self) {
        if self.shared.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = self.shared.clone();
        let handle = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || shared.run());
        match handle {
            Ok(h) => *lock(&self.thread) = Some(h),
            Err(e) => {
                self.shared.running.store(false, Ordering::SeqCst);
                log_msg!(LogLevel::Error, "audio: Thread startet nicht: {}", e);
            }
        }
    }

    pub fn stop(&self) {
        if !self.shared.running.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Some(h) = lock(&self.thread).take() {
            let _ = h.join();
        }
    }

    // Alles Folgende ist thread-sicher.

    pub fn configure(&self, mode: Mode, smooth_delay_ms: u32) {
        let mut c = lock(&self.shared.cfg);
        c.mode = mode;
        c.delay_ms = smooth_delay_ms;
        c.dirty = true;
    }

    pub fn set_gain(&self, gain: f32) {
        self.shared.gain.store(gain.to_bits(), Ordering::SeqCst);
        self.shared.gain_dirty.store(true, Ordering::SeqCst);
    }

    /// Ersatz, solange kein Ton läuft.
    pub fn set_video_lead_ns(&self, lead_ns: i64) {
        self.shared.video_lead_ns.store(lead_ns, Ordering::SeqCst);
    }

    /// Neue Sitzung: Decoder, Puffer, Messung zurücksetzen.
    pub fn request_reset(&self) {
        self.shared.reset_requested.store(true, Ordering::SeqCst);
    }

    pub fn set_pcm_tap(&self, tap: Option<PcmTap>) {
        *lock(&self.shared.tap) = tap;
    }

    pub fn present_offset_ns(&self) -> i64 {
        self.shared.offset_ns.load(Ordering::SeqCst)
    }

    pub fn stats(&self) -> AudioStats {
        *lock(&self.shared.stats)
    }

    /// Eine Runde (läuft im Thread; öffentlich für Tests).
    pub fn step(&self) {
        self.shared.step();
    }
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Shared {
    fn run(&self) {
        // SAFETY: Pseudo-Handle des eigenen Threads, keine weiteren Vorbedingungen.
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
        while self.running.load(Ordering::SeqCst) {
            self.step();
            std::thread::sleep(STEP_PAUSE);
        }
    }

    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::SeqCst))
    }

    fn step(&self) {
        let mut guard = lock(&self.worker);
        let w = &mut *guard;
        let sink = &*self.sink;

        if self.reset_requested.swap(false, Ordering::SeqCst) {
            w.decoder.close();
            w.pending.clear();
            w.sched.reset();
            w.lead.reset();
            w.offsets.reset();
            sink.clear();
        }
        {
            let mut c = lock(&self.cfg);
            if c.dirty {
                c.dirty = false;
                w.active_mode = c.mode;
                w.active_delay_ms = c.delay_ms;
                w.sched.configure(w.active_mode, w.active_delay_ms, RATE);
                w.offsets.reset();
                self.offset_ns
                    .store(i64::from(w.active_delay_ms) * 1_000_000, Ordering::SeqCst);
            }
        }
        if self.gain_dirty.swap(false, Ordering::SeqCst) && sink.is_open() {
            sink.set_gain(self.gain());
        }

        // 1. Ankommende Pakete dekodieren und vormerken
        for p in self.source.drain() {
            if p.ct == 0 {
                continue; // Format noch unbekannt
            }
            if p.arrival_ns != 0 {
                w.lead
                    .add((p.ntp_local as i64).wrapping_sub(p.arrival_ns as i64));
            }
            if (!w.decoder.is_open() || w.decoder.ct() != p.ct) && !w.decoder.open(p.ct) {
                continue;
            }
            let pcm = w.decoder.decode(&p);
            if pcm.is_empty() {
                continue;
            }
            if !sink.is_open() {
                if !sink.open(RATE, 2) {
                    continue;
                }
                sink.set_gain(self.gain());
            }
            let frames = pcm.len() / 2;
            if let Some(tap) = lock(&self.tap).as_mut() {
                tap(&pcm, p.ntp_local);
            }
            w.pending.push(&pcm);
            w.sched.push(p.ntp_local, frames);
        }

        // 2. "Flüssig": Gesamtverzögerung ab Ankunft → Vorlauf des Senders abziehen
        if w.active_mode == Mode::Smooth {
            let video_lead = self.video_lead_ns.load(Ordering::SeqCst);
            let lead = if w.lead.known() {
                w.lead.median()
            } else if video_lead != i64::MIN {
                video_lead
            } else {
                0
            };
            let offset = w.offsets.update(w.active_delay_ms, lead);
            w.sched.set_present_offset(offset);
            self.offset_ns.store(offset, Ordering::SeqCst);
        }

        // 3. Freigeben/Verwerfen/Kürzen nach Plan
        if !sink.is_open() {
            return;
        }
        let now = (w.clock)();
        let plan = w.sched.plan(now, sink.queued_frames());
        if plan.discard_pending_frames > 0 {
            w.pending.drop_oldest(plan.discard_pending_frames);
        }
        if plan.release_frames > 0 {
            let mut buf = vec![0.0f32; plan.release_frames * 2];
            let n = w.pending.pop(&mut buf);
            sink.push(&buf[..n * 2]);
        }
        if plan.drop_queued_frames > 0 {
            sink.drop_oldest(plan.drop_queued_frames);
        }
        sink.set_rate_ratio(plan.rate_ratio);
        w.released += plan.release_frames;
        w.discarded += plan.discard_pending_frames;
        w.dropped += plan.drop_queued_frames;

        let stats = {
            let mut s = lock(&self.stats);
            s.buffer_ms = frames_to_ms(sink.queued_frames());
            s.pending_ms = frames_to_ms(w.pending.frames());
            s.ratio = plan.rate_ratio;
            s.audio_lead_ms = if w.lead.known() {
                w.lead.median() as f32 / 1e6
            } else {
                0.0
            };
            *s
        };
        if now.wrapping_sub(w.last_diag_ns) >= DIAG_INTERVAL_NS {
            w.last_diag_ns = now;
            log_msg!(
                LogLevel::Info,
                "diag-audio: modus={} puffer={:.0}ms wartend={:.0}ms frei={:.0}ms verw={:.0}ms \
                 gekürzt={:.0}ms tempo={:.3} vorlauf={:.0}ms versatz={:.0}ms",
                if w.active_mode == Mode::Smooth {
                    "fluessig"
                } else {
                    "niedrig"
                },
                stats.buffer_ms,
                stats.pending_ms,
                frames_to_ms(w.released),
                frames_to_ms(w.discarded),
                frames_to_ms(w.dropped),
                plan.rate_ratio,
                stats.audio_lead_ms,
                self.offset_ns.load(Ordering::SeqCst) as f64 / 1e6
            );
            w.released = 0;
            w.discarded = 0;
            w.dropped = 0;
        }
    }
}
