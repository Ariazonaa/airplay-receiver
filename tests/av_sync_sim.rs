//! Port von test_av_sync_sim.cpp.
//!
//! Simulation mit den im Log gemessenen Werten (2026-11-19, iPhone19,7):
//! Ton kommt ~183 ms vor seinem Zeitstempel an, Bild ~97 ms, Schwankung einige ms,
//! einmal 2,6 s Funkpause mit anschließendem Schwall. Zusätzlich läuft die
//! PC-Soundkarte 0,1 % langsamer als die Uhr des iPhones.

use std::collections::VecDeque;

use airplay::app::config::Mode;
use airplay::media::scheduler::{AudioScheduler, FrameScheduler, LeadEstimator, OffsetTracker};

const MS: i64 = 1_000_000;
const RATE: i32 = 44100;
/// AAC-ELD spf
const BLOCK: usize = 480;
/// Soundkarte etwas langsamer
const OUTPUT_RATE: f64 = RATE as f64 * 0.999;

/// `srand`/`rand` der Windows-CRT (v1 lief mit UCRT), damit die Zufallsfolge dieselbe ist.
struct CrtRand(u32);

impl CrtRand {
    fn new(seed: u32) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> i64 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as i64
    }
}

/// Nachbau von libstdc++ `std::sort` (v1 lief mit GCC/UCRT64). `std::sort` ist nicht stabil:
/// Die Reihenfolge gleicher Ankunftszeiten im Schwall bestimmt, wo sich der Versatz danach
/// einpendelt (Hysterese 15 ms). Mit einer stabilen Sortierung springt er erst bei t = 16 s
/// – im ausgewerteten Bereich. Darum hier genau dieselbe Reihenfolge wie v1.
mod gcc_sort {
    const THRESHOLD: usize = 16;

    pub fn sort<T: Copy>(v: &mut [T], less: impl Fn(&T, &T) -> bool) {
        let n = v.len();
        if n == 0 {
            return;
        }
        let lg = (usize::BITS - 1 - n.leading_zeros()) as usize;
        introsort_loop(v, 0, n, lg * 2, &less);
        final_insertion_sort(v, 0, n, &less);
    }

    fn introsort_loop<T: Copy>(
        v: &mut [T],
        first: usize,
        mut last: usize,
        mut depth: usize,
        less: &impl Fn(&T, &T) -> bool,
    ) {
        while last - first > THRESHOLD {
            if depth == 0 {
                make_heap(v, first, last, less);
                sort_heap(v, first, last, less);
                return;
            }
            depth -= 1;
            let cut = partition_pivot(v, first, last, less);
            introsort_loop(v, cut, last, depth, less);
            last = cut;
        }
    }

    fn partition_pivot<T: Copy>(
        v: &mut [T],
        first: usize,
        last: usize,
        less: &impl Fn(&T, &T) -> bool,
    ) -> usize {
        let mid = first + (last - first) / 2;
        move_median_to_first(v, first, first + 1, mid, last - 1, less);
        unguarded_partition(v, first + 1, last, first, less)
    }

    fn move_median_to_first<T: Copy>(
        v: &mut [T],
        result: usize,
        a: usize,
        b: usize,
        c: usize,
        less: &impl Fn(&T, &T) -> bool,
    ) {
        let l = |x: usize, y: usize| less(&v[x], &v[y]);
        let pick = if l(a, b) {
            if l(b, c) {
                b
            } else if l(a, c) {
                c
            } else {
                a
            }
        } else if l(a, c) {
            a
        } else if l(b, c) {
            c
        } else {
            b
        };
        v.swap(result, pick);
    }

    fn unguarded_partition<T: Copy>(
        v: &mut [T],
        mut first: usize,
        mut last: usize,
        pivot: usize,
        less: &impl Fn(&T, &T) -> bool,
    ) -> usize {
        loop {
            while less(&v[first], &v[pivot]) {
                first += 1;
            }
            last -= 1;
            while less(&v[pivot], &v[last]) {
                last -= 1;
            }
            if first >= last {
                return first;
            }
            v.swap(first, last);
            first += 1;
        }
    }

    fn final_insertion_sort<T: Copy>(
        v: &mut [T],
        first: usize,
        last: usize,
        less: &impl Fn(&T, &T) -> bool,
    ) {
        if last - first > THRESHOLD {
            insertion_sort(v, first, first + THRESHOLD, less);
            for i in first + THRESHOLD..last {
                unguarded_linear_insert(v, i, less);
            }
        } else {
            insertion_sort(v, first, last, less);
        }
    }

    fn insertion_sort<T: Copy>(
        v: &mut [T],
        first: usize,
        last: usize,
        less: &impl Fn(&T, &T) -> bool,
    ) {
        if first == last {
            return;
        }
        for i in first + 1..last {
            if less(&v[i], &v[first]) {
                let val = v[i];
                v.copy_within(first..i, first + 1);
                v[first] = val;
            } else {
                unguarded_linear_insert(v, i, less);
            }
        }
    }

    fn unguarded_linear_insert<T: Copy>(
        v: &mut [T],
        mut last: usize,
        less: &impl Fn(&T, &T) -> bool,
    ) {
        let val = v[last];
        let mut next = last - 1;
        while less(&val, &v[next]) {
            v[last] = v[next];
            last = next;
            next -= 1;
        }
        v[last] = val;
    }

    fn make_heap<T: Copy>(v: &mut [T], first: usize, last: usize, less: &impl Fn(&T, &T) -> bool) {
        let len = last - first;
        if len < 2 {
            return;
        }
        let mut parent = (len - 2) / 2;
        loop {
            let value = v[first + parent];
            adjust_heap(v, first, parent, len, value, less);
            if parent == 0 {
                return;
            }
            parent -= 1;
        }
    }

    fn sort_heap<T: Copy>(
        v: &mut [T],
        first: usize,
        mut last: usize,
        less: &impl Fn(&T, &T) -> bool,
    ) {
        while last - first > 1 {
            last -= 1;
            let value = v[last];
            v[last] = v[first];
            adjust_heap(v, first, 0, last - first, value, less);
        }
    }

    fn adjust_heap<T: Copy>(
        v: &mut [T],
        first: usize,
        mut hole: usize,
        len: usize,
        value: T,
        less: &impl Fn(&T, &T) -> bool,
    ) {
        let top = hole;
        let mut second = hole;
        while second < (len - 1) / 2 {
            second = 2 * (second + 1);
            if less(&v[first + second], &v[first + second - 1]) {
                second -= 1;
            }
            v[first + hole] = v[first + second];
            hole = second;
        }
        if len & 1 == 0 && second == (len - 2) / 2 {
            second = 2 * (second + 1);
            v[first + hole] = v[first + second - 1];
            hole = second - 1;
        }
        // push_heap
        let mut parent = hole.wrapping_sub(1) / 2;
        while hole > top && less(&v[first + parent], &value) {
            v[first + hole] = v[first + parent];
            hole = parent;
            parent = hole.wrapping_sub(1) / 2;
        }
        v[first + hole] = value;
    }
}

#[derive(Clone, Copy)]
struct Pkt {
    arrival: i64,
    ts: i64,
}

struct SimResult {
    /// Ausgabepuffer nach Einschwingen
    max_queue_ms: f64,
    min_queue_ms: f64,
    min_queue_at_ms: f64,
    /// |Abspielzeit − Plan| (nur Flüssig)
    max_audio_err_ms: f64,
    /// |Anzeige − Plan| (nur Flüssig)
    max_video_err_ms: f64,
    /// Ton: Ankunft → Abspielen
    max_arrival_to_play_ms: f64,
}

fn settled(now: i64) -> bool {
    now > 4000 * MS && !(now > 10000 * MS && now < 16000 * MS)
}

fn simulate(mode: Mode, delay_ms: u32, seconds: i64) -> SimResult {
    let mut rng = CrtRand::new(42);
    let mut audio = Vec::new();
    let mut video = Vec::new();
    let end = seconds * 1000 * MS;
    // Pakete erzeugen (Senderzeit t)
    let audio_step = BLOCK as i64 * 1_000_000_000 / RATE as i64;
    let mut t = 0i64;
    while t < end {
        let mut arrival = t + (rng.next() % 12) * MS;
        if (10000 * MS..12600 * MS).contains(&t) {
            arrival = 12600 * MS + (rng.next() % 5) * MS; // Funkpause + Schwall
        }
        audio.push(Pkt {
            arrival,
            ts: t + 183 * MS,
        });
        t += audio_step;
    }
    let mut t = 0i64;
    while t < end {
        let mut arrival = t + (rng.next() % 8) * MS;
        if (10000 * MS..12600 * MS).contains(&t) {
            arrival = 12600 * MS + (rng.next() % 5) * MS;
        }
        video.push(Pkt {
            arrival,
            ts: t + 97 * MS,
        });
        t += 16_666_667;
    }
    gcc_sort::sort(&mut audio, |a: &Pkt, b: &Pkt| a.arrival < b.arrival);
    gcc_sort::sort(&mut video, |a: &Pkt, b: &Pkt| a.arrival < b.arrival);
    let mut audio: VecDeque<Pkt> = audio.into();
    let mut video: VecDeque<Pkt> = video.into();

    let mut as_ = AudioScheduler::default();
    let mut fs = FrameScheduler::default();
    as_.configure(mode, delay_ms, RATE);
    fs.configure(mode, delay_ms);
    let mut audio_lead = LeadEstimator::default();
    let mut video_lead = LeadEstimator::default();
    let mut offsets = OffsetTracker::default();
    let mut offset = delay_ms as i64 * MS;

    let mut pending: VecDeque<(i64, usize)> = VecDeque::new(); // ts, frames (Zwischenpuffer)
    let mut out_frames = 0.0f64;
    let mut ratio = 1.0f64;
    let mut r = SimResult {
        max_queue_ms: 0.0,
        min_queue_ms: 1e9,
        min_queue_at_ms: 0.0,
        max_audio_err_ms: 0.0,
        max_video_err_ms: 0.0,
        max_arrival_to_play_ms: 0.0,
    };
    let mut next_id = 0u64;
    let mut id_to_ts: Vec<i64> = Vec::new();

    let mut now = 0i64;
    while now < end {
        while let Some(p) = audio.front().copied().filter(|p| p.arrival <= now) {
            audio_lead.add(p.ts - now);
            as_.push(p.ts as u64, BLOCK);
            pending.push_back((p.ts, BLOCK));
            audio.pop_front();
        }
        while let Some(p) = video.front().copied().filter(|p| p.arrival <= now) {
            video_lead.add(p.ts - now);
            fs.push(p.ts as u64, next_id);
            next_id += 1;
            id_to_ts.push(p.ts);
            video.pop_front();
        }
        // Wie App::scheduleAudio: Gesamtverzögerung = Sender-Vorlauf + Offset
        if mode == Mode::Smooth {
            let lead = if audio_lead.known() {
                audio_lead.median()
            } else if video_lead.known() {
                video_lead.median()
            } else {
                0
            };
            offset = offsets.update(delay_ms, lead);
            as_.set_present_offset(offset);
            fs.set_present_offset(offset);
        }
        let p = as_.plan(now as u64, out_frames as usize);
        let mut d = p.discard_pending_frames;
        while d > 0 {
            let Some(blk) = pending.pop_front() else {
                break;
            };
            d = d.saturating_sub(blk.1);
        }
        let mut rel = p.release_frames;
        while rel > 0 {
            let Some(blk) = pending.pop_front() else {
                break;
            };
            rel = rel.saturating_sub(blk.1);
            let play_at_ms = now as f64 / 1e6 + out_frames * 1000.0 / OUTPUT_RATE / ratio;
            if settled(now) {
                r.max_arrival_to_play_ms = r
                    .max_arrival_to_play_ms
                    .max(play_at_ms - (blk.0 - 183 * MS) as f64 / 1e6);
                if mode == Mode::Smooth {
                    let due_ms = (blk.0 + offset) as f64 / 1e6;
                    r.max_audio_err_ms = r.max_audio_err_ms.max((play_at_ms - due_ms).abs());
                }
            }
            out_frames += blk.1 as f64;
        }
        if p.drop_queued_frames > 0 {
            out_frames = (out_frames - p.drop_queued_frames as f64).max(0.0);
        }
        ratio = f64::from(p.rate_ratio);

        let picked = fs.pick(now as u64);
        if let Some(id) = picked
            && mode == Mode::Smooth
            && settled(now)
        {
            let due_ms = (id_to_ts[id as usize] + offset) as f64 / 1e6;
            r.max_video_err_ms = r.max_video_err_ms.max((now as f64 / 1e6 - due_ms).abs());
        }

        out_frames = (out_frames - OUTPUT_RATE * ratio / 1000.0).max(0.0);
        if settled(now) {
            let q = out_frames * 1000.0 / RATE as f64;
            r.max_queue_ms = r.max_queue_ms.max(q);
            if q < r.min_queue_ms {
                r.min_queue_ms = q;
                r.min_queue_at_ms = now as f64 / 1e6;
            }
        }
        now += MS;
    }
    r
}

#[test]
fn sim_smooth_200_ms_keeps_total_delay_near_200_ms_and_in_sync() {
    let r = simulate(Mode::Smooth, 200, 30);
    println!(
        "Puffer {} (bei {} ms)–{} ms, Ankunft→Ton max {} ms, Ton-Fehler {} ms, Bild-Fehler {} ms",
        r.min_queue_ms,
        r.min_queue_at_ms,
        r.max_queue_ms,
        r.max_arrival_to_play_ms,
        r.max_audio_err_ms,
        r.max_video_err_ms
    );
    assert!(r.max_arrival_to_play_ms < 260.0); // vorher ~480 ms
    assert!(r.min_queue_ms > 20.0); // kein Leerlauf (vorher Aussetzer durch Planlücken)
    assert!(r.max_queue_ms < 260.0);
    assert!(r.max_audio_err_ms < 30.0); // Ton läuft im Plan
    assert!(r.max_video_err_ms < 20.0); // Bild läuft im Plan → Lippensynchron
}

#[test]
fn sim_smooth_100_ms_works_with_sender_lead_above_the_delay() {
    let r = simulate(Mode::Smooth, 100, 30);
    println!(
        "Puffer {}–{} ms, Ankunft→Ton max {} ms",
        r.min_queue_ms, r.max_queue_ms, r.max_arrival_to_play_ms
    );
    assert!(r.max_arrival_to_play_ms < 160.0);
    assert!(r.min_queue_ms > 20.0);
    assert!(r.max_audio_err_ms < 30.0);
}

#[test]
fn sim_low_latency_keeps_buffer_small_despite_drift_and_burst() {
    let r = simulate(Mode::LowLatency, 200, 30);
    println!(
        "Puffer {}–{} ms, Ankunft→Ton max {} ms",
        r.min_queue_ms, r.max_queue_ms, r.max_arrival_to_play_ms
    );
    assert!(r.max_queue_ms < 125.0);
    assert!(r.max_arrival_to_play_ms < 130.0);
}
