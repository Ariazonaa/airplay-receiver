//! FFmpeg-Decoder für H.264/H.265. Mit Device: D3D11VA (Frames bleiben auf der GPU,
//! Format `AV_PIX_FMT_D3D11`). Ohne Device oder wenn das scheitert: Software.

use std::ffi::{CStr, c_char, c_int};
use std::ptr::{NonNull, null_mut};

use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::core::Interface;

use crate::app::log::LogLevel;
use crate::ffi::ffmpeg as av;
use crate::log_msg;
use crate::protocol::packets::VideoPacket;

/// App hält bis zu 40 Bilder fest (`MAX_PENDING_FRAMES` in `app::app`).
const EXTRA_HW_FRAMES: c_int = 44;
/// `AVERROR(EAGAIN)`; EAGAIN ist unter der Windows-CRT 11.
const AVERROR_EAGAIN: c_int = -11;
/// `AV_NOPTS_VALUE` (Makro, von bindgen nicht übernommen)
const AV_NOPTS_VALUE: i64 = i64::MIN;
const MAX_LOGGED_ERRORS: u32 = 20;

/// Ein dekodiertes Bild (eigene Referenz auf ein `AVFrame`, wird per `av_frame_free` freigegeben).
pub struct Frame {
    ptr: NonNull<av::AVFrame>,
}

// SAFETY: Das Frame gehört allein dieser Hülle; FFmpegs Puffer sind referenzgezählt und
// threadsicher, D3D11-Flächen liegen auf einem multithread-geschützten Device.
unsafe impl Send for Frame {}

impl Frame {
    /// Übernimmt `ptr` (darf null sein → `None`).
    fn from_raw(ptr: *mut av::AVFrame) -> Option<Frame> {
        NonNull::new(ptr).map(|ptr| Frame { ptr })
    }

    fn raw(&self) -> &av::AVFrame {
        // SAFETY: ptr ist gültig, solange das Frame lebt.
        unsafe { self.ptr.as_ref() }
    }

    /// Roher Zeiger für FFI-Hüllen (Renderer, Aufnahme); gültig, solange `self` lebt.
    pub fn as_ptr(&self) -> *const av::AVFrame {
        self.ptr.as_ptr()
    }

    pub fn width(&self) -> i32 {
        self.raw().width
    }

    pub fn height(&self) -> i32 {
        self.raw().height
    }

    pub fn format(&self) -> av::AVPixelFormat {
        self.raw().format
    }

    /// GPU-Fläche (`AV_PIX_FMT_D3D11`)
    pub fn is_hw(&self) -> bool {
        self.format() == av::AV_PIX_FMT_D3D11
    }

    /// µs aus `VideoPacket::ntp_local / 1000`, falls vorhanden
    pub fn pts(&self) -> Option<i64> {
        let pts = self.raw().pts;
        (pts != AV_NOPTS_VALUE).then_some(pts)
    }

    /// Neue Referenz auf dieselben Daten (`av_frame_clone`).
    ///
    /// # Panics
    /// Nur wenn FFmpeg keinen Speicher mehr bekommt.
    pub fn clone_ref(&self) -> Frame {
        // SAFETY: ptr ist ein gültiges AVFrame.
        let p = unsafe { av::av_frame_clone(self.ptr.as_ptr()) };
        Frame::from_raw(p).expect("av_frame_clone: kein Speicher")
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let mut p = self.ptr.as_ptr();
        // SAFETY: Wir besitzen die Referenz; av_frame_free setzt p auf null.
        unsafe { av::av_frame_free(&mut p) };
    }
}

/// `get_format`: D3D11, wenn ein Hardware-Device gesetzt ist, sonst das erste Software-Format.
unsafe extern "C" fn pick_format(
    ctx: *mut av::AVCodecContext,
    formats: *const av::AVPixelFormat,
) -> av::AVPixelFormat {
    if formats.is_null() {
        return av::AV_PIX_FMT_NONE;
    }
    // SAFETY: FFmpeg übergibt ein gültiges Codec-Objekt und eine mit AV_PIX_FMT_NONE
    // abgeschlossene Liste.
    unsafe {
        let has_hw = !ctx.is_null() && !(*ctx).hw_device_ctx.is_null();
        let mut p = formats;
        while *p != av::AV_PIX_FMT_NONE {
            if *p == av::AV_PIX_FMT_D3D11 && has_hw {
                return *p;
            }
            p = p.add(1);
        }
        // Keine Hardware: erstes Software-Format.
        let mut p = formats;
        while *p != av::AV_PIX_FMT_NONE {
            let d = av::av_pix_fmt_desc_get(*p);
            if !d.is_null() && (*d).flags & u64::from(av::AV_PIX_FMT_FLAG_HWACCEL) == 0 {
                return *p;
            }
            p = p.add(1);
        }
    }
    av::AV_PIX_FMT_NONE
}

fn codec_name(h265: bool) -> &'static str {
    if h265 { "H.265" } else { "H.264" }
}

fn av_error_text(rc: c_int) -> String {
    let mut buf = [0 as c_char; 128];
    // SAFETY: buf ist groß genug und wird nullterminiert.
    unsafe {
        av::av_strerror(rc, buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

pub struct VideoDecoder {
    ctx: *mut av::AVCodecContext,
    hw_device: *mut av::AVBufferRef,
    h265: bool,
    hardware: bool,
    width: i32,
    height: i32,
    errors_logged: u32,
}

impl Default for VideoDecoder {
    fn default() -> Self {
        Self {
            ctx: null_mut(),
            hw_device: null_mut(),
            h265: false,
            hardware: false,
            width: 0,
            height: 0,
            errors_logged: 0,
        }
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        self.close();
    }
}

impl VideoDecoder {
    /// Öffnet den Decoder neu. Mit `device` wird D3D11VA auf genau diesem Device versucht
    /// (FFmpeg teilt es mit dem Renderer), sonst bzw. bei Fehler Software.
    pub fn open(&mut self, device: Option<&ID3D11Device>, h265: bool) -> bool {
        self.close();
        self.h265 = h265;
        // SAFETY: FFmpeg-Aufrufe mit eigenen, hier angelegten Objekten; Zeiger werden vor
        // Benutzung geprüft.
        unsafe {
            let codec = av::avcodec_find_decoder(if h265 {
                av::AV_CODEC_ID_HEVC
            } else {
                av::AV_CODEC_ID_H264
            });
            if codec.is_null() {
                log_msg!(
                    LogLevel::Error,
                    "video: kein Decoder für {}",
                    codec_name(h265)
                );
                return false;
            }
            self.ctx = av::avcodec_alloc_context3(codec);
            if self.ctx.is_null() {
                log_msg!(LogLevel::Error, "video: avcodec_open2 fehlgeschlagen");
                return false;
            }
            let ctx = &mut *self.ctx;
            ctx.flags |= av::AV_CODEC_FLAG_LOW_DELAY as c_int;
            ctx.flags2 |= av::AV_CODEC_FLAG2_FAST as c_int;
            ctx.thread_count = 1;
            ctx.get_format = Some(pick_format);

            if let Some(device) = device {
                self.attach_d3d11va(device);
            }

            if av::avcodec_open2(self.ctx, codec, null_mut()) < 0 {
                log_msg!(LogLevel::Error, "video: avcodec_open2 fehlgeschlagen");
                self.close();
                return false;
            }
            self.hardware = !(*self.ctx).hw_device_ctx.is_null();
        }
        log_msg!(
            LogLevel::Info,
            "video: Decoder {} geöffnet ({})",
            codec_name(h265),
            if self.hardware { "D3D11VA" } else { "Software" }
        );
        true
    }

    /// Hardware-Kontext auf dem Device des Renderers; bei Fehler bleibt es bei Software.
    unsafe fn attach_d3d11va(&mut self, device: &ID3D11Device) {
        // SAFETY: self.ctx ist gültig (open); hw ist ein frisch angelegter D3D11VA-Kontext,
        // dessen hwctx ein AVD3D11VADeviceContext ist.
        unsafe {
            let mut hw = av::av_hwdevice_ctx_alloc(av::AV_HWDEVICE_TYPE_D3D11VA);
            if hw.is_null() {
                log_msg!(
                    LogLevel::Warn,
                    "video: D3D11VA-Init fehlgeschlagen, nutze Software"
                );
                return;
            }
            let hwctx = (*hw).data.cast::<av::AVHWDeviceContext>();
            let d3d = &mut *(*hwctx).hwctx.cast::<av::AVD3D11VADeviceContext>();
            // FFmpeg gibt beide Referenzen beim Freigeben wieder ab (clone = AddRef).
            d3d.device = device.clone().into_raw().cast();
            if let Ok(context) = device.GetImmediateContext() {
                d3d.device_context = context.into_raw().cast();
            }
            if av::av_hwdevice_ctx_init(hw) < 0 {
                log_msg!(
                    LogLevel::Warn,
                    "video: D3D11VA-Init fehlgeschlagen, nutze Software"
                );
                av::av_buffer_unref(&mut hw);
            } else {
                (*self.ctx).hw_device_ctx = av::av_buffer_ref(hw);
                (*self.ctx).extra_hw_frames = EXTRA_HW_FRAMES;
                self.hw_device = hw;
            }
        }
    }

    pub fn close(&mut self) {
        // SAFETY: Beide Funktionen setzen die Zeiger auf null und vertragen null.
        unsafe {
            if !self.ctx.is_null() {
                av::avcodec_free_context(&mut self.ctx);
            }
            if !self.hw_device.is_null() {
                av::av_buffer_unref(&mut self.hw_device);
            }
        }
        self.hardware = false;
        self.errors_logged = 0;
    }

    pub fn is_open(&self) -> bool {
        !self.ctx.is_null()
    }

    pub fn is_h265(&self) -> bool {
        self.h265
    }

    pub fn hardware(&self) -> bool {
        self.hardware
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// Ein Paket (Access Unit) dekodieren; liefert alle fertigen Bilder.
    pub fn decode(&mut self, packet: &VideoPacket) -> Vec<Frame> {
        let mut out = Vec::new();
        if self.ctx.is_null() || packet.data.is_empty() {
            return out;
        }
        let Ok(len) = c_int::try_from(packet.data.len()) else {
            return out;
        };
        // SAFETY: self.ctx ist geöffnet; pkt wird hier angelegt und freigegeben, sein Puffer
        // ist len Bytes groß.
        unsafe {
            let mut pkt = av::av_packet_alloc();
            if pkt.is_null() {
                return out;
            }
            if av::av_new_packet(pkt, len) == 0 {
                std::ptr::copy_nonoverlapping(packet.data.as_ptr(), (*pkt).data, packet.data.len());
                (*pkt).pts = (packet.ntp_local / 1000) as i64; // µs, nur zur Zuordnung
                let rc = av::avcodec_send_packet(self.ctx, pkt);
                if rc < 0 && rc != AVERROR_EAGAIN && self.errors_logged < MAX_LOGGED_ERRORS {
                    self.errors_logged += 1;
                    log_msg!(LogLevel::Warn, "video: send_packet: {}", av_error_text(rc));
                }
                loop {
                    let Some(f) = Frame::from_raw(av::av_frame_alloc()) else {
                        break;
                    };
                    if av::avcodec_receive_frame(self.ctx, f.ptr.as_ptr()) < 0 {
                        break;
                    }
                    self.width = f.width();
                    self.height = f.height();
                    self.hardware = f.is_hw();
                    out.push(f);
                }
            }
            av::av_packet_free(&mut pkt);
        }
        out
    }
}
