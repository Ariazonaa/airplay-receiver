//! AAC-ELD (ct=8) bzw. AAC-LC (ct=4) → interleaved float, 44 100 Hz, Stereo.

use std::ffi::{CStr, c_char, c_int};
use std::ptr::null_mut;

use crate::app::log::LogLevel;
use crate::ffi::ffmpeg as av;
use crate::log_msg;
use crate::protocol::packets::AudioPacket;

/// `AVERROR(EAGAIN)`; EAGAIN ist unter der Windows-CRT 11.
const AVERROR_EAGAIN: c_int = -11;
const MAX_LOGGED_ERRORS: u32 = 20;
const RATE: c_int = 44100;
const CHANNELS: c_int = 2;

/// AirPlay-Lautstärke (dB, −30…0, −144 = stumm) → linearer Faktor 0…1.
pub fn volume_to_gain(db: f32) -> f32 {
    if db <= -144.0 {
        return 0.0;
    }
    10f32.powf(db / 20.0).clamp(0.0, 1.0)
}

fn av_error_text(rc: c_int) -> String {
    let mut buf = [0 as c_char; 128];
    // SAFETY: buf ist groß genug und wird nullterminiert.
    unsafe {
        av::av_strerror(rc, buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

pub struct AudioDecoder {
    ctx: *mut av::AVCodecContext,
    swr: *mut av::SwrContext,
    ct: u8,
    errors_logged: u32,
}

// SAFETY: Die FFmpeg-Kontexte gehören allein dieser Hülle und werden nie gleichzeitig aus
// mehreren Threads benutzt (`&mut self`); FFmpeg erlaubt den Wechsel des Threads.
unsafe impl Send for AudioDecoder {}

impl Default for AudioDecoder {
    fn default() -> Self {
        Self {
            ctx: null_mut(),
            swr: null_mut(),
            ct: 0,
            errors_logged: 0,
        }
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        self.close();
    }
}

impl AudioDecoder {
    pub fn open(&mut self, ct: u8) -> bool {
        // AudioSpecificConfig wie UxPlays audio_renderer.c (ISO 14496-3 1.6.2.1).
        match ct {
            8 => {
                self.ct = 8;
                // AAC-ELD 44.1k/2, spf 480
                self.open_codec(av::AV_CODEC_ID_AAC, &[0xF8, 0xE8, 0x50, 0x00])
            }
            4 => {
                self.ct = 4;
                // AAC-LC 44.1k/2, spf 1024
                self.open_codec(av::AV_CODEC_ID_AAC, &[0x12, 0x10])
            }
            _ => {
                self.close();
                log_msg!(
                    LogLevel::Warn,
                    "audio: Format ct={} wird nicht unterstützt",
                    ct
                );
                false
            }
        }
    }

    /// Nur für Tests: beliebiger Codec mit eigener Extradata.
    pub fn open_for_test(&mut self, codec_id: av::AVCodecID, extradata: Vec<u8>) -> bool {
        self.ct = 0;
        self.open_codec(codec_id, &extradata)
    }

    fn open_codec(&mut self, codec_id: av::AVCodecID, extradata: &[u8]) -> bool {
        let keep_ct = self.ct;
        self.close();
        self.ct = keep_ct;
        let Ok(extra_len) = c_int::try_from(extradata.len()) else {
            return false;
        };
        // SAFETY: FFmpeg-Aufrufe mit hier angelegten Objekten; Zeiger werden vor Benutzung
        // geprüft. extradata bekommt den von FFmpeg verlangten, genullten Überhang.
        unsafe {
            let codec = av::avcodec_find_decoder(codec_id);
            if codec.is_null() {
                return false;
            }
            self.ctx = av::avcodec_alloc_context3(codec);
            if self.ctx.is_null() {
                return false;
            }
            let ctx = &mut *self.ctx;
            ctx.sample_rate = RATE;
            av::av_channel_layout_default(&mut ctx.ch_layout, CHANNELS);
            if !extradata.is_empty() {
                let buf =
                    av::av_mallocz(extradata.len() + av::AV_INPUT_BUFFER_PADDING_SIZE as usize)
                        .cast::<u8>();
                if buf.is_null() {
                    self.close();
                    return false;
                }
                std::ptr::copy_nonoverlapping(extradata.as_ptr(), buf, extradata.len());
                ctx.extradata = buf;
                ctx.extradata_size = extra_len;
            }
            if av::avcodec_open2(self.ctx, codec, null_mut()) < 0 {
                log_msg!(LogLevel::Error, "audio: Decoder öffnet nicht");
                self.close();
                return false;
            }
        }
        log_msg!(LogLevel::Info, "audio: Decoder geöffnet (ct={})", self.ct);
        true
    }

    pub fn close(&mut self) {
        // SAFETY: Beide Funktionen setzen die Zeiger auf null und vertragen null.
        unsafe {
            if !self.ctx.is_null() {
                av::avcodec_free_context(&mut self.ctx);
            }
            if !self.swr.is_null() {
                av::swr_free(&mut self.swr);
            }
        }
        self.ct = 0;
        self.errors_logged = 0;
    }

    pub fn is_open(&self) -> bool {
        !self.ctx.is_null()
    }

    pub fn ct(&self) -> u8 {
        self.ct
    }

    pub fn sample_rate(&self) -> i32 {
        RATE
    }

    pub fn channels(&self) -> i32 {
        CHANNELS
    }

    /// Hängt den dekodierten Block `f` als Stereo-Float (44,1 kHz) an `out` an.
    ///
    /// # Safety
    /// `f` ist ein gültiges, dekodiertes Audio-`AVFrame`.
    unsafe fn convert(&mut self, f: *const av::AVFrame, out: &mut Vec<f32>) {
        // SAFETY: f ist gültig (Vorbedingung); swr wird hier angelegt und geprüft; dst zeigt
        // auf max_out · 2 freie Floats.
        unsafe {
            let f = &*f;
            if self.swr.is_null() {
                let mut out_layout: av::AVChannelLayout = std::mem::zeroed();
                av::av_channel_layout_default(&mut out_layout, CHANNELS);
                if av::swr_alloc_set_opts2(
                    &mut self.swr,
                    &out_layout,
                    av::AV_SAMPLE_FMT_FLT,
                    RATE,
                    &f.ch_layout,
                    f.format,
                    f.sample_rate,
                    0,
                    null_mut(),
                ) < 0
                    || av::swr_init(self.swr) < 0
                {
                    log_msg!(LogLevel::Error, "audio: swresample-Init fehlgeschlagen");
                    if !self.swr.is_null() {
                        av::swr_free(&mut self.swr);
                    }
                    return;
                }
            }
            let max_out = av::swr_get_out_samples(self.swr, f.nb_samples);
            if max_out <= 0 {
                return;
            }
            let base = out.len();
            out.resize(base + max_out as usize * 2, 0.0);
            let dst = out.as_mut_ptr().add(base).cast::<u8>();
            let n = av::swr_convert(
                self.swr,
                &dst,
                max_out,
                f.extended_data as *const *const u8,
                f.nb_samples,
            );
            out.truncate(base + n.max(0) as usize * 2);
        }
    }

    /// Ein Paket dekodieren; liefert interleaved Stereo-Float (leer bei Fehler).
    pub fn decode(&mut self, packet: &AudioPacket) -> Vec<f32> {
        let mut out = Vec::new();
        if self.ctx.is_null() || packet.data.is_empty() {
            return out;
        }
        let Ok(len) = c_int::try_from(packet.data.len()) else {
            return out;
        };
        // SAFETY: self.ctx ist geöffnet; pkt/frame werden hier angelegt und freigegeben,
        // der Paketpuffer ist len Bytes groß.
        unsafe {
            let mut pkt = av::av_packet_alloc();
            if pkt.is_null() {
                return out;
            }
            if av::av_new_packet(pkt, len) == 0 {
                std::ptr::copy_nonoverlapping(packet.data.as_ptr(), (*pkt).data, packet.data.len());
                let rc = av::avcodec_send_packet(self.ctx, pkt);
                if rc < 0 && rc != AVERROR_EAGAIN && self.errors_logged < MAX_LOGGED_ERRORS {
                    self.errors_logged += 1;
                    log_msg!(LogLevel::Warn, "audio: send_packet: {}", av_error_text(rc));
                }
                let mut f = av::av_frame_alloc();
                if !f.is_null() {
                    while av::avcodec_receive_frame(self.ctx, f) == 0 {
                        self.convert(f, &mut out);
                        av::av_frame_unref(f);
                    }
                    av::av_frame_free(&mut f);
                }
            }
            av::av_packet_free(&mut pkt);
        }
        out
    }
}
