//! Begrenzte, thread-sichere Queue. Ist sie voll, verwirft `push()` das älteste Element und
//! zählt es in `dropped()`. `push()` blockiert nie (für UxPlay-Callbacks) und panikt nie.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

struct Inner<T> {
    items: VecDeque<T>,
    dropped: u64,
}

pub struct PacketQueue<T> {
    inner: Mutex<Inner<T>>,
    capacity: usize,
}

impl<T> PacketQueue<T> {
    /// Kapazität 0 wird zu 1.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            inner: Mutex::new(Inner {
                items: VecDeque::new(),
                dropped: 0,
            }),
            capacity,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner<T>> {
        // Eine Panik in einem anderen Thread darf die Queue nicht unbrauchbar machen.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn push(&self, value: T) {
        let mut g = self.lock();
        if g.items.len() >= self.capacity {
            g.items.pop_front();
            g.dropped += 1;
        }
        g.items.push_back(value);
    }

    pub fn pop(&self) -> Option<T> {
        self.lock().items.pop_front()
    }

    pub fn drain(&self) -> Vec<T> {
        self.lock().items.drain(..).collect()
    }

    pub fn clear(&self) {
        self.lock().items.clear();
    }

    pub fn len(&self) -> usize {
        self.lock().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().items.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn dropped(&self) -> u64 {
        self.lock().dropped
    }
}
