//! Port von test_scheduler.cpp.

#[macro_use]
#[path = "support/approx.rs"]
mod approx;

use airplay::app::config::Mode;
use airplay::media::scheduler::{AudioScheduler, FrameScheduler, LeadEstimator, OffsetTracker};

const fn ms(v: u64) -> u64 {
    v * 1_000_000
}

#[test]
fn low_latency_picks_newest_and_drops_older() {
    let mut s = FrameScheduler::default();
    s.configure(Mode::LowLatency, 200);
    s.push(ms(0), 1);
    s.push(ms(16), 2);
    s.push(ms(33), 3);
    assert_eq!(s.pick(ms(33)), Some(3));
    assert_eq!(s.dropped(), 2);
    assert!(s.pick(ms(40)).is_none());
}

#[test]
fn smooth_waits_for_delay() {
    let mut s = FrameScheduler::default();
    s.configure(Mode::Smooth, 200);
    s.push(ms(0), 7);
    assert!(s.pick(ms(199)).is_none());
    assert_eq!(s.pick(ms(200)), Some(7));
    assert_eq!(s.dropped(), 0);
}

#[test]
fn smooth_picks_newest_due_frame_and_keeps_future_ones() {
    let mut s = FrameScheduler::default();
    s.configure(Mode::Smooth, 200);
    s.push(ms(0), 1);
    s.push(ms(16), 2);
    s.push(ms(100), 3);
    assert_eq!(s.pick(ms(220)), Some(2));
    assert_eq!(s.dropped(), 1);
    assert_eq!(s.pick(ms(300)), Some(3));
}

#[test]
fn smooth_drops_stale() {
    let mut s = FrameScheduler::default();
    s.configure(Mode::Smooth, 200);
    s.push(ms(0), 1);
    assert!(s.pick(ms(301)).is_none());
    assert_eq!(s.dropped(), 1);
}

#[test]
fn mode_switch_releases_backlog() {
    let mut s = FrameScheduler::default();
    s.configure(Mode::Smooth, 200);
    s.push(ms(1000), 1);
    s.push(ms(1016), 2);
    s.push(ms(1033), 3);
    assert!(s.pick(ms(1040)).is_none());
    s.configure(Mode::LowLatency, 200);
    assert_eq!(s.pick(ms(1040)), Some(3));
}

#[test]
fn reset_forgets_frames() {
    let mut s = FrameScheduler::default();
    s.configure(Mode::Smooth, 200);
    s.push(ms(0), 1);
    s.reset();
    assert!(s.pick(ms(500)).is_none());
}

#[test]
fn audio_low_latency_trims_big_spikes_to_60ms() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::LowLatency, 200, 44100);
    let p = a.plan(ms(0), 8000); // ~181 ms
    assert_eq!(p.drop_queued_frames, 8000 - 2646);
}

#[test]
fn audio_low_latency_does_not_cut_moderate_excess() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::LowLatency, 200, 44100);
    assert_eq!(a.plan(ms(0), 3000).drop_queued_frames, 0);
    assert_eq!(a.plan(ms(0), 5000).drop_queued_frames, 0); // ~113 ms: wird weich abgebaut
    assert_eq!(a.plan(ms(0), 5292).drop_queued_frames, 0); // genau 120 ms
}

#[test]
fn audio_low_latency_speeds_up_gently_above_target() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::LowLatency, 200, 44100);
    assert_approx!(a.plan(ms(0), 2646).rate_ratio, 1.0f32); // 60 ms = Ziel
    assert_approx!(a.plan(ms(0), 3528).rate_ratio, 1.01f32); // 80 ms
    assert_approx!(a.plan(ms(0), 4410).rate_ratio, 1.02f32); // 100 ms
    assert_approx!(a.plan(ms(0), 5292).rate_ratio, 1.02f32); // begrenzt auf +2 %
    assert_approx!(a.plan(ms(0), 1764).rate_ratio, 0.99f32); // 40 ms: etwas langsamer
    assert_approx!(a.plan(ms(0), 441).rate_ratio, 0.98f32); // 10 ms: begrenzt auf -2 %
}

#[test]
fn lead_estimator_uses_median_and_ignores_bursts() {
    let mut e = LeadEstimator::default();
    assert!(!e.known());
    for _ in 0..50 {
        e.add(ms(183) as i64);
    }
    for _ in 0..5 {
        e.add(-(ms(2600) as i64)); // Schwall verspäteter Pakete
    }
    assert!(e.known());
    assert_eq!(e.median(), ms(183) as i64);
}

#[test]
fn smooth_with_present_offset_releases_at_ts_offset() {
    // Gesamtverzögerung 200 ms, Sender-Vorlauf 183 ms → nur noch 17 ms auf den Zeitstempel.
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 200, 44100);
    a.set_present_offset(ms(17) as i64);
    a.push(ms(1000), 441);
    // Ton darf 20 ms vor Fälligkeit (1017) in die Ausgabe, damit keine Lücken entstehen.
    assert_eq!(a.plan(ms(996), 0).release_frames, 0);
    assert_eq!(a.plan(ms(997), 0).release_frames, 441);

    let mut f = FrameScheduler::default();
    f.configure(Mode::Smooth, 200);
    f.set_present_offset(ms(17) as i64);
    f.push(ms(1000), 1);
    assert!(f.pick(ms(1016)).is_none());
    assert_eq!(f.pick(ms(1017)), Some(1));
}

#[test]
fn smooth_with_negative_present_offset() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 100, 44100);
    a.set_present_offset(-(ms(83) as i64));
    a.push(ms(1000), 441);
    assert_eq!(a.plan(ms(896), 0).release_frames, 0); // fällig 917, 20 ms Spielraum
    assert_eq!(a.plan(ms(897), 0).release_frames, 441);
}

#[test]
fn smooth_audio_corrects_excess_buffer_back_to_schedule() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 200, 44100);
    a.set_present_offset(0);
    a.push(ms(1000), 441); // 10 ms Block, fällig bei 1000
    assert_eq!(a.plan(ms(1000), 0).release_frames, 441);
    // Laut Plan endet das Freigegebene bei 1010 ms → bei now=1000 erwartet: 10 ms in der Ausgabe.
    assert_approx!(a.plan(ms(1000), 441).rate_ratio, 1.0f32);
    // 30 ms mehr als geplant → etwas schneller abspielen.
    assert_approx!(a.plan(ms(1000), 441 + 1323).rate_ratio, 1.015f32);
    // Weit über Plan (> 120 ms) → hart auf Plan kürzen.
    let p = a.plan(ms(1000), 441 + 8820); // +200 ms
    assert_eq!(p.drop_queued_frames, 8820);
}

#[test]
fn smooth_releases_a_block_slightly_after_the_queue_end_instead_of_starving() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 200, 44100);
    a.set_present_offset(0);
    a.push(ms(1155), 480);
    // Ausgabe reicht noch 146 ms (bis 1146), Block fällig bei 1155 → 9 ms Lücke: trotzdem freigeben.
    assert_eq!(a.plan(ms(1000), 6438).release_frames, 480);
}

#[test]
fn smooth_does_not_release_blocks_far_ahead() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 200, 44100);
    a.set_present_offset(0);
    a.push(ms(1200), 480);
    assert_eq!(a.plan(ms(1000), 6438).release_frames, 0); // 54 ms Lücke: warten
}

#[test]
fn offset_tracker_ignores_small_lead_jitter() {
    let mut t = OffsetTracker::default();
    assert_eq!(t.update(200, ms(183) as i64), ms(17) as i64);
    assert_eq!(t.update(200, ms(176) as i64), ms(17) as i64); // 7 ms: bleibt
    assert_eq!(t.update(200, ms(190) as i64), ms(17) as i64); // 7 ms: bleibt
    assert_eq!(t.update(200, ms(160) as i64), ms(40) as i64); // 23 ms: nachführen
    assert_eq!(t.update(300, ms(160) as i64), ms(140) as i64); // Verzögerung geändert: sofort
    t.reset();
    assert_eq!(t.update(200, ms(100) as i64), ms(100) as i64);
}

#[test]
fn audio_smooth_plays_at_normal_speed() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 200, 44100);
    assert_approx!(a.plan(ms(0), 5000).rate_ratio, 1.0f32);
}

#[test]
fn audio_low_latency_releases_immediately_while_playing() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::LowLatency, 200, 44100);
    a.push(ms(1000), 480);
    a.push(ms(1011), 480);
    assert_eq!(a.plan(ms(0), 2000).release_frames, 960); // Ausgabe läuft (45 ms)
    assert_eq!(a.plan(ms(0), 2000).release_frames, 0);
}

#[test]
fn audio_low_latency_prebuffers_60_ms_when_output_is_empty() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::LowLatency, 200, 44100);
    for i in 0..5 {
        a.push(ms(1000 + i * 11), 480); // 2400 Frames ≈ 54 ms
    }
    assert_eq!(a.plan(ms(0), 0).release_frames, 0); // sammeln
    a.push(ms(1055), 480); // 2880 Frames ≈ 65 ms
    assert_eq!(a.plan(ms(0), 0).release_frames, 2880); // jetzt am Stück
}

#[test]
fn audio_smooth_holds_until_due() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 200, 44100);
    a.push(ms(0), 441);
    assert_eq!(a.plan(ms(150), 0).release_frames, 0);
    assert_eq!(a.plan(ms(200), 0).release_frames, 441);
}

#[test]
fn audio_smooth_counts_output_latency() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 200, 44100);
    a.push(ms(0), 441);
    // 2205 Frames in der Ausgabe = 50 ms: Block wird 50 ms früher freigegeben.
    assert_eq!(a.plan(ms(150), 2205).release_frames, 441);
}

#[test]
fn audio_smooth_discards_stale_blocks() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 200, 44100);
    a.push(ms(0), 441);
    a.push(ms(500), 441);
    let p = a.plan(ms(350), 0);
    assert_eq!(p.discard_pending_frames, 441);
    assert_eq!(p.release_frames, 0);
}

#[test]
fn audio_switch_smooth_to_low_latency_releases_backlog() {
    let mut a = AudioScheduler::default();
    a.configure(Mode::Smooth, 200, 44100);
    a.push(ms(1000), 441);
    a.push(ms(1010), 441);
    assert_eq!(a.plan(ms(1000), 0).release_frames, 0);
    a.configure(Mode::LowLatency, 200, 44100);
    assert_eq!(a.plan(ms(1000), 2000).release_frames, 882); // Ausgabe läuft
}
