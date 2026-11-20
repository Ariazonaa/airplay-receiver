//! Ton-Ausgabe: Ringpuffer, Ziel-Schnittstelle und SDL3-Ausgabe (WASAPI, Standardgerät).

use std::collections::VecDeque;
use std::ffi::{CStr, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::app::log::LogLevel;
use crate::ffi::sdl3::audio::{
    SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK, SDL_AUDIO_F32, SDL_AudioSpec, SDL_AudioStream,
    SDL_ClearAudioStream, SDL_DestroyAudioStream, SDL_GetAudioStreamQueued,
    SDL_OpenAudioDeviceStream, SDL_PutAudioStreamData, SDL_ResumeAudioStreamDevice,
    SDL_SetAudioStreamFrequencyRatio, SDL_SetAudioStreamGain,
};
use crate::ffi::sdl3::error::SDL_GetError;
use crate::ffi::sdl3::init::{SDL_INIT_AUDIO, SDL_InitSubSystem, SDL_WasInit};
use crate::log_msg;

/// Bytes je Stereo-Float-Frame
const FRAME_BYTES: usize = std::mem::size_of::<f32>() * 2;

/// Stereo-Float-Ringpuffer (Frames = Sample-Paare). Nicht thread-sicher.
#[derive(Debug, Default)]
pub struct SampleRing {
    samples: VecDeque<f32>,
}

impl SampleRing {
    /// Hängt alle ganzen Frames aus `interleaved` an.
    pub fn push(&mut self, interleaved: &[f32]) {
        let n = interleaved.len() / 2 * 2;
        self.samples.extend(&interleaved[..n]);
    }

    /// Füllt `out` (Platz für `out.len() / 2` Frames); liefert die Anzahl Frames.
    pub fn pop(&mut self, out: &mut [f32]) -> usize {
        let n = (out.len() / 2).min(self.frames());
        for (dst, src) in out.iter_mut().zip(self.samples.drain(..n * 2)) {
            *dst = src;
        }
        n
    }

    pub fn drop_oldest(&mut self, frames: usize) {
        let n = frames.min(self.frames()) * 2;
        self.samples.drain(..n);
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

/// Ziel für dekodierten Ton (Stereo-Float). Implementierungen müssen thread-sicher sein
/// (geteilt zwischen Ton-Thread und z. B. SDLs Audio-Callback).
pub trait AudioSink: Send + Sync {
    fn open(&self, rate: i32, channels: i32) -> bool;
    fn is_open(&self) -> bool;
    /// interleaved Stereo-Float
    fn push(&self, interleaved: &[f32]);
    fn queued_frames(&self) -> usize;
    fn drop_oldest(&self, frames: usize);
    fn clear(&self);
    fn set_gain(&self, gain: f32);
    /// Abspieltempo (1.0 = normal)
    fn set_rate_ratio(&self, ratio: f32);
}

struct StreamPtr(*mut SDL_AudioStream);

// SAFETY: SDL-Audio-Streams sind thread-sicher (eigene Sperre); der Zeiger wird nur unter
// dem Mutex in `AudioOutput` gelesen und geändert.
unsafe impl Send for StreamPtr {}

fn sdl_error() -> String {
    let e = SDL_GetError();
    if e.is_null() {
        return String::new();
    }
    // SAFETY: SDL liefert einen nullterminierten String.
    unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ausgabe über SDL3 (WASAPI) auf dem Standardgerät. Der Puffer liegt bei uns, damit der
/// Scheduler gezielt kürzen kann; SDL holt sich Daten per Callback.
pub struct AudioOutput {
    stream: Mutex<StreamPtr>,
    /// Auf dem Heap, damit der Callback-Zeiger stabil bleibt, auch wenn `AudioOutput` bewegt wird.
    ring: Box<Mutex<SampleRing>>,
    rate: AtomicI32,
}

impl Default for AudioOutput {
    fn default() -> Self {
        Self {
            stream: Mutex::new(StreamPtr(null_mut())),
            ring: Box::new(Mutex::new(SampleRing::default())),
            rate: AtomicI32::new(44100),
        }
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        self.close();
    }
}

/// SDL fragt aus seinem Audio-Thread nach weiteren Bytes. Fehlt Material, bleibt es still.
/// Darf nicht paniken.
unsafe extern "C" fn feed(
    userdata: *mut c_void,
    stream: *mut SDL_AudioStream,
    additional: c_int,
    _total: c_int,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if userdata.is_null() || additional <= 0 {
            return;
        }
        // SAFETY: userdata ist der Ring aus `AudioOutput::open`; er lebt, bis der Stream in
        // `close` zerstört ist.
        let ring = unsafe { &*(userdata as *const Mutex<SampleRing>) };
        let wanted = additional as usize / FRAME_BYTES;
        let mut buf = vec![0.0f32; wanted * 2];
        let got = lock(ring).pop(&mut buf);
        if got > 0 {
            // SAFETY: stream ist der aufrufende Stream; buf enthält got Frames.
            unsafe {
                SDL_PutAudioStreamData(stream, buf.as_ptr().cast(), (got * FRAME_BYTES) as c_int)
            };
        }
    }));
}

impl AudioOutput {
    pub fn close(&self) {
        {
            let mut s = lock(&self.stream);
            if !s.0.is_null() {
                // SAFETY: Stream aus SDL_OpenAudioDeviceStream; danach nicht mehr benutzt.
                // SDL wartet einen laufenden Callback ab.
                unsafe { SDL_DestroyAudioStream(s.0) };
                s.0 = null_mut();
            }
        }
        lock(&self.ring).clear();
    }

    /// Nur unser Puffer (ohne SDL-intern), für Diagnose.
    pub fn ring_frames(&self) -> usize {
        lock(&self.ring).frames()
    }

    pub fn rate(&self) -> i32 {
        self.rate.load(Ordering::Relaxed)
    }

    fn with_stream(&self, f: impl FnOnce(*mut SDL_AudioStream)) {
        let s = lock(&self.stream);
        if !s.0.is_null() {
            f(s.0);
        }
    }
}

impl AudioSink for AudioOutput {
    fn open(&self, rate: i32, channels: i32) -> bool {
        self.close();
        // SAFETY: SDL-Aufrufe ohne weitere Vorbedingungen; spec lebt während des Aufrufs,
        // userdata (Ring) lebt länger als der Stream (Drop ruft close).
        unsafe {
            if (SDL_WasInit(SDL_INIT_AUDIO).0 & SDL_INIT_AUDIO.0) == 0
                && !SDL_InitSubSystem(SDL_INIT_AUDIO)
            {
                log_msg!(LogLevel::Error, "audio: SDL-Audio: {}", sdl_error());
                return false;
            }
            let spec = SDL_AudioSpec {
                format: SDL_AUDIO_F32,
                channels,
                freq: rate,
            };
            let userdata = (&*self.ring as *const Mutex<SampleRing>).cast_mut().cast();
            let stream = SDL_OpenAudioDeviceStream(
                SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK,
                &spec,
                Some(feed),
                userdata,
            );
            if stream.is_null() {
                log_msg!(
                    LogLevel::Error,
                    "audio: Ausgabegerät öffnet nicht: {}",
                    sdl_error()
                );
                return false;
            }
            lock(&self.stream).0 = stream;
            self.rate.store(rate, Ordering::Relaxed);
            SDL_ResumeAudioStreamDevice(stream);
        }
        log_msg!(
            LogLevel::Info,
            "audio: Ausgabe geöffnet ({} Hz, {} Kanäle)",
            rate,
            channels
        );
        true
    }

    fn is_open(&self) -> bool {
        !lock(&self.stream).0.is_null()
    }

    fn push(&self, interleaved: &[f32]) {
        lock(&self.ring).push(interleaved);
    }

    fn queued_frames(&self) -> usize {
        let mut sdl_frames = 0;
        self.with_stream(|s| {
            // SAFETY: gültiger Stream (unter dem Mutex).
            let bytes = unsafe { SDL_GetAudioStreamQueued(s) };
            sdl_frames = bytes.max(0) as usize / FRAME_BYTES;
        });
        lock(&self.ring).frames() + sdl_frames
    }

    fn drop_oldest(&self, frames: usize) {
        lock(&self.ring).drop_oldest(frames);
    }

    fn clear(&self) {
        // Ring und SDL-Stream nacheinander sperren: der Callback hält die Stream-Sperre und
        // will dann den Ring – beides zugleich zu halten könnte sich gegenseitig blockieren.
        lock(&self.ring).clear();
        // SAFETY: gültiger Stream (unter dem Mutex).
        self.with_stream(|s| unsafe {
            SDL_ClearAudioStream(s);
        });
    }

    fn set_gain(&self, gain: f32) {
        // SAFETY: gültiger Stream (unter dem Mutex).
        self.with_stream(|s| unsafe {
            SDL_SetAudioStreamGain(s, gain);
        });
    }

    fn set_rate_ratio(&self, ratio: f32) {
        // SAFETY: gültiger Stream (unter dem Mutex).
        self.with_stream(|s| unsafe {
            SDL_SetAudioStreamFrequencyRatio(s, ratio);
        });
    }
}
