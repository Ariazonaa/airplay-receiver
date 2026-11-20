//! Port von test_packet_queue.cpp.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::thread;

use airplay::protocol::packet_queue::PacketQueue;
use airplay::protocol::receiver::Receiver;

#[test]
fn fifo_order() {
    let q = PacketQueue::new(10);
    q.push(1);
    q.push(2);
    q.push(3);
    assert_eq!(q.pop(), Some(1));
    assert_eq!(q.pop(), Some(2));
    assert_eq!(q.len(), 1);
    assert_eq!(q.pop(), Some(3));
    assert_eq!(q.pop(), None);
}

#[test]
fn full_queue_drops_oldest_and_counts() {
    let q = PacketQueue::new(3);
    for i in 1..=5 {
        q.push(i);
    }
    assert_eq!(q.drain(), vec![3, 4, 5]);
    assert_eq!(q.dropped(), 2);
    assert_eq!(q.len(), 0);
}

#[test]
fn clear_empties() {
    let q = PacketQueue::new(3);
    q.push(1);
    q.push(2);
    q.clear();
    assert_eq!(q.len(), 0);
    assert_eq!(q.pop(), None);
}

#[test]
fn concurrent_push_pop_loses_nothing_but_counted_drops() {
    const PER_THREAD: i64 = 10000;
    let q = Arc::new(PacketQueue::new(64));
    let popped = Arc::new(AtomicI64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let producer = |q: Arc<PacketQueue<i64>>| {
        thread::spawn(move || {
            for _ in 0..PER_THREAD {
                q.push(1);
            }
        })
    };
    let a = producer(q.clone());
    let b = producer(q.clone());
    let consumer = {
        let (q, popped, done) = (q.clone(), popped.clone(), done.clone());
        thread::spawn(move || {
            while !done.load(Ordering::SeqCst) || !q.is_empty() {
                if let Some(v) = q.pop() {
                    popped.fetch_add(v, Ordering::SeqCst);
                }
            }
        })
    };
    a.join().unwrap();
    b.join().unwrap();
    done.store(true, Ordering::SeqCst);
    consumer.join().unwrap();
    assert_eq!(
        popped.load(Ordering::SeqCst) + q.dropped() as i64,
        2 * PER_THREAD
    );
}

#[test]
fn zero_capacity_becomes_one() {
    let q = PacketQueue::new(0);
    assert_eq!(q.capacity(), 1);
    q.push(1);
    q.push(2);
    assert_eq!(q.drain(), vec![2]);
    assert_eq!(q.dropped(), 1);
}

#[test]
fn receiver_video_queue_survives_a_20_s_stall_at_60_fps() {
    // Verworfene komprimierte Pakete zerstören das Bild bis zum nächsten Keyframe (bei AirPlay selten).
    let r = Receiver::new();
    assert!(r.video().capacity() >= 20 * 60);
    assert_eq!(r.video().capacity(), 1800);
    assert_eq!(r.audio().capacity(), 400);
    assert_eq!(r.events().capacity(), 64);
    assert!(!r.running());
}
