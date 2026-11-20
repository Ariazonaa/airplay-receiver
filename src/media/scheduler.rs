//! Zeitplanung für Bild und Ton (reine Logik, Zeiten in ns auf `Receiver::now_ns()`-Basis).
//! Algorithmen und Konstanten wie v1 `scheduler.cpp` – live mit dem Nutzer abgestimmt.

use std::collections::VecDeque;

use crate::app::config::Mode;

const MS_NS: i64 = 1_000_000;
const SEC_NS: i64 = 1_000_000_000;
const STALE_NS: i64 = 100 * MS_NS;
const LOW_TARGET_MS: usize = 60;
/// darunter gilt die Ausgabe als leer
const EMPTY_MS: usize = 5;
/// darüber hart kürzen (beide Modi)
const HARD_MAX_MS: usize = 120;
/// +1 % Tempo je 20 ms Abweichung
const RATIO_PER_MS: f32 = 0.01 / 20.0;
const MAX_RATIO_DELTA: f32 = 0.02;
const LEAD_WINDOW: usize = 128;
const LEAD_KNOWN: usize = 16;
const OFFSET_HYSTERESIS_NS: i64 = 15 * MS_NS;
/// Block darf so viel nach Pufferende fällig sein
const RELEASE_SLACK_NS: i64 = 20 * MS_NS;
const DEFAULT_OFFSET_NS: i64 = 200 * MS_NS;

fn ratio_for(error_ms: f32) -> f32 {
    1.0 + (error_ms * RATIO_PER_MS).clamp(-MAX_RATIO_DELTA, MAX_RATIO_DELTA)
}

/// Vorlauf des Senders: um wie viel ns ein Paket vor seinem Zeitstempel ankommt
/// (iOS plant mit eigenem Puffer; gemessen ~180 ms Ton, ~100 ms Bild). Median über
/// die letzten 128 Werte, damit Schübe verspäteter Pakete nicht durchschlagen.
#[derive(Debug, Default)]
pub struct LeadEstimator {
    values: VecDeque<i64>,
}

impl LeadEstimator {
    pub fn add(&mut self, lead_ns: i64) {
        self.values.push_back(lead_ns);
        if self.values.len() > LEAD_WINDOW {
            self.values.pop_front();
        }
    }

    pub fn known(&self) -> bool {
        self.values.len() >= LEAD_KNOWN
    }

    pub fn median(&self) -> i64 {
        if self.values.is_empty() {
            return 0;
        }
        let mut v: Vec<i64> = self.values.iter().copied().collect();
        let mid = v.len() / 2;
        *v.select_nth_unstable(mid).1
    }

    pub fn reset(&mut self) {
        self.values.clear();
    }
}

/// Versatz für "Flüssig": offset = Verzögerung − Sender-Vorlauf. Kleine Schwankungen
/// des Vorlaufs (≤ 15 ms) werden ignoriert, sonst entstehen Lücken im Ton-Zeitplan.
#[derive(Debug, Default)]
pub struct OffsetTracker {
    set: bool,
    delay_ms: u32,
    offset_ns: i64,
}

impl OffsetTracker {
    pub fn update(&mut self, delay_ms: u32, lead_ns: i64) -> i64 {
        let target = i64::from(delay_ms) * MS_NS - lead_ns;
        if !self.set
            || delay_ms != self.delay_ms
            || (target - self.offset_ns).abs() > OFFSET_HYSTERESIS_NS
        {
            self.set = true;
            self.delay_ms = delay_ms;
            self.offset_ns = target;
        }
        self.offset_ns
    }

    pub fn reset(&mut self) {
        self.set = false;
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    ts: u64,
    id: u64,
}

/// Welches Bild wann gezeigt wird.
///  LowLatency: immer das neueste Bild, ältere gelten als verworfen.
///  Smooth:     neuestes Bild mit ts + offset <= now; mehr als 100 ms überfällige werden verworfen.
#[derive(Debug)]
pub struct FrameScheduler {
    mode: Mode,
    offset_ns: i64,
    frames: VecDeque<Entry>,
    dropped: u64,
}

impl Default for FrameScheduler {
    fn default() -> Self {
        Self {
            mode: Mode::LowLatency,
            offset_ns: DEFAULT_OFFSET_NS,
            frames: VecDeque::new(),
            dropped: 0,
        }
    }
}

impl FrameScheduler {
    pub fn configure(&mut self, mode: Mode, smooth_delay_ms: u32) {
        self.mode = mode;
        self.offset_ns = i64::from(smooth_delay_ms) * MS_NS;
    }

    /// Smooth: gezeigt wird bei ts + offset (Standard nach `configure`: Verzögerung).
    pub fn set_present_offset(&mut self, offset_ns: i64) {
        self.offset_ns = offset_ns;
    }

    pub fn push(&mut self, ts_ns: u64, frame_id: u64) {
        self.frames.push_back(Entry {
            ts: ts_ns,
            id: frame_id,
        });
    }

    pub fn pick(&mut self, now_ns: u64) -> Option<u64> {
        let last = self.frames.back()?.id;
        if self.mode == Mode::LowLatency {
            self.dropped += self.frames.len() as u64 - 1;
            self.frames.clear();
            return Some(last);
        }
        // Smooth: zu späte Bilder verwerfen, dann das neueste fällige zeigen.
        let now = now_ns as i64;
        let offset = self.offset_ns;
        let due = |e: &Entry| e.ts as i64 + offset;
        while self.frames.front().is_some_and(|e| due(e) + STALE_NS < now) {
            self.frames.pop_front();
            self.dropped += 1;
        }
        let mut chosen = None;
        while let Some(e) = self.frames.front().copied().filter(|e| due(e) <= now) {
            if chosen.is_some() {
                self.dropped += 1;
            }
            chosen = Some(e.id);
            self.frames.pop_front();
        }
        chosen
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn pending(&self) -> usize {
        self.frames.len()
    }

    pub fn reset(&mut self) {
        self.frames.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioPlan {
    /// aus dem Zwischenpuffer an die Ausgabe geben
    pub release_frames: usize,
    /// älteste Frames im Zwischenpuffer verwerfen (zu spät)
    pub discard_pending_frames: usize,
    /// älteste Frames in der Ausgabe verwerfen (Puffer zu voll)
    pub drop_queued_frames: usize,
    /// Abspieltempo der Ausgabe (weiche Driftkorrektur)
    pub rate_ratio: f32,
}

impl Default for AudioPlan {
    fn default() -> Self {
        Self {
            release_frames: 0,
            discard_pending_frames: 0,
            drop_queued_frames: 0,
            rate_ratio: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Block {
    ts: u64,
    frames: usize,
}

///  LowLatency: alles sofort freigeben; Tempo ±2 % regelt den Puffer weich auf 60 ms
///              (+1 % je 20 ms Abweichung); nur Ausreißer > 120 ms werden hart auf 60 ms gekürzt.
///  Smooth:     Blöcke freigeben, sobald ts + offset <= now + Ausgabelatenz;
///              mehr als 100 ms überfällige Blöcke verwerfen; Ausgabepuffer weich (±2 %)
///              auf den Zeitplan regeln, > 120 ms Überschuss hart kürzen.
#[derive(Debug)]
pub struct AudioScheduler {
    mode: Mode,
    offset_ns: i64,
    rate: i32,
    blocks: VecDeque<Block>,
    /// Plan-Ende des zuletzt freigegebenen Tons (Smooth); −1 = noch nichts freigegeben
    released_due_end_ns: i64,
}

impl Default for AudioScheduler {
    fn default() -> Self {
        Self {
            mode: Mode::LowLatency,
            offset_ns: DEFAULT_OFFSET_NS,
            rate: 44100,
            blocks: VecDeque::new(),
            released_due_end_ns: -1,
        }
    }
}

impl AudioScheduler {
    pub fn configure(&mut self, mode: Mode, smooth_delay_ms: u32, rate: i32) {
        self.mode = mode;
        self.offset_ns = i64::from(smooth_delay_ms) * MS_NS;
        self.rate = if rate > 0 { rate } else { 44100 };
        self.released_due_end_ns = -1;
    }

    pub fn set_present_offset(&mut self, offset_ns: i64) {
        self.offset_ns = offset_ns;
    }

    pub fn push(&mut self, ts_ns: u64, frames: usize) {
        self.blocks.push_back(Block { ts: ts_ns, frames });
    }

    pub fn plan(&mut self, now_ns: u64, queued_frames: usize) -> AudioPlan {
        let mut p = AudioPlan::default();
        let rate = self.rate as usize;
        let hard_max_frames = rate * HARD_MAX_MS / 1000;
        if self.mode == Mode::LowLatency {
            let target = rate * LOW_TARGET_MS / 1000;
            let pending_frames: usize = self.blocks.iter().map(|b| b.frames).sum();
            // Ausgabe leer (Start oder Aussetzer): erst 60 ms sammeln, dann am Stück abspielen.
            if queued_frames < rate * EMPTY_MS / 1000 && pending_frames < target {
                return p;
            }
            p.release_frames = pending_frames;
            self.blocks.clear();
            if queued_frames > hard_max_frames {
                p.drop_queued_frames = queued_frames - target;
                return p;
            }
            p.rate_ratio =
                ratio_for((queued_frames as f32 - target as f32) * 1000.0 / self.rate as f32);
            return p;
        }

        let now = now_ns as i64;
        let rate_i = i64::from(self.rate);
        let frames_to_ns = |frames: i64| frames * SEC_NS / rate_i;
        let mut output_ns = frames_to_ns(queued_frames as i64);

        // Ausgabepuffer gegen den Zeitplan prüfen: was laut Plan noch zu spielen ist.
        if self.released_due_end_ns >= 0 {
            let expected_ns = (self.released_due_end_ns - now).max(0);
            let error_ns = output_ns - expected_ns;
            if error_ns > HARD_MAX_MS as i64 * MS_NS {
                p.drop_queued_frames = (error_ns * rate_i / SEC_NS) as usize;
                output_ns -= frames_to_ns(p.drop_queued_frames as i64);
            } else {
                p.rate_ratio = ratio_for(error_ns as f32 / MS_NS as f32);
            }
        }

        let offset = self.offset_ns;
        let due = |b: &Block| b.ts as i64 + offset;
        while let Some(b) = self
            .blocks
            .front()
            .copied()
            .filter(|b| due(b) + STALE_NS < now)
        {
            p.discard_pending_frames += b.frames;
            self.blocks.pop_front();
        }
        while let Some(b) = self
            .blocks
            .front()
            .copied()
            .filter(|b| due(b) <= now + output_ns + RELEASE_SLACK_NS)
        {
            p.release_frames += b.frames;
            let duration = frames_to_ns(b.frames as i64);
            self.released_due_end_ns = due(&b) + duration;
            output_ns += duration;
            self.blocks.pop_front();
        }
        p
    }

    pub fn reset(&mut self) {
        self.blocks.clear();
        self.released_due_end_ns = -1;
    }
}
