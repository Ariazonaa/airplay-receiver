//! Bild in voller Auflösung als PNG speichern (D3D11-Frames werden vorher in den RAM kopiert).

use std::path::Path;
use std::ptr::{null, null_mut};

use crate::app::log::LogLevel;
use crate::ffi::ffmpeg as av;
use crate::log_msg;
use crate::media::video_decoder::Frame;

/// Gibt die angelegten FFmpeg-Objekte in jedem Fall frei.
struct Resources {
    cpu: *mut av::AVFrame,
    rgb: *mut av::AVFrame,
    enc: *mut av::AVCodecContext,
    sws: *mut av::SwsContext,
    pkt: *mut av::AVPacket,
}

impl Drop for Resources {
    fn drop(&mut self) {
        // SAFETY: Alle Zeiger sind null oder gehören allein dieser Hülle; die Freigabe-
        // funktionen vertragen null bzw. setzen auf null.
        unsafe {
            av::av_packet_free(&mut self.pkt);
            if !self.sws.is_null() {
                av::sws_freeContext(self.sws);
            }
            av::avcodec_free_context(&mut self.enc);
            av::av_frame_free(&mut self.rgb);
            av::av_frame_free(&mut self.cpu);
        }
    }
}

/// Fehler beim Kodieren; v1 meldet ein unlesbares GPU-Bild ohne "Speichern fehlgeschlagen".
enum EncodeError {
    GpuUnreadable,
    Failed,
}

/// PNG kodieren.
fn encode_png(f: &Frame) -> Result<Vec<u8>, EncodeError> {
    let mut r = Resources {
        cpu: null_mut(),
        rgb: null_mut(),
        enc: null_mut(),
        sws: null_mut(),
        pkt: null_mut(),
    };
    // SAFETY: `f` ist ein gültiges AVFrame; alle anderen Objekte werden hier angelegt, vor
    // Benutzung geprüft und von `Resources` freigegeben.
    unsafe {
        let src = f.as_ptr();
        let mut input = src;
        if !(*src).hw_frames_ctx.is_null() {
            r.cpu = av::av_frame_alloc();
            if r.cpu.is_null() || av::av_hwframe_transfer_data(r.cpu, src, 0) < 0 {
                log_msg!(LogLevel::Error, "screenshot: GPU-Bild nicht lesbar");
                return Err(EncodeError::GpuUnreadable);
            }
            input = r.cpu;
        }
        let (w, h) = ((*input).width, (*input).height);
        r.rgb = av::av_frame_alloc();
        if r.rgb.is_null() {
            return Err(EncodeError::Failed);
        }
        (*r.rgb).format = av::AV_PIX_FMT_RGB24;
        (*r.rgb).width = w;
        (*r.rgb).height = h;
        let codec = av::avcodec_find_encoder(av::AV_CODEC_ID_PNG);
        if !codec.is_null() {
            r.enc = av::avcodec_alloc_context3(codec);
        }
        r.sws = av::sws_getContext(
            w,
            h,
            (*input).format,
            w,
            h,
            av::AV_PIX_FMT_RGB24,
            av::SWS_BICUBIC as i32,
            null_mut(),
            null_mut(),
            null(),
        );
        if r.enc.is_null() || r.sws.is_null() || av::av_frame_get_buffer(r.rgb, 0) != 0 {
            return Err(EncodeError::Failed);
        }
        av::sws_scale(
            r.sws,
            (*input).data.as_ptr().cast::<*const u8>(),
            (*input).linesize.as_ptr(),
            0,
            h,
            (*r.rgb).data.as_ptr(),
            (*r.rgb).linesize.as_ptr(),
        );
        (*r.enc).width = w;
        (*r.enc).height = h;
        (*r.enc).pix_fmt = av::AV_PIX_FMT_RGB24;
        (*r.enc).time_base = av::AVRational { num: 1, den: 25 };
        r.pkt = av::av_packet_alloc();
        if r.pkt.is_null()
            || av::avcodec_open2(r.enc, codec, null_mut()) != 0
            || av::avcodec_send_frame(r.enc, r.rgb) != 0
            || av::avcodec_receive_packet(r.enc, r.pkt) != 0
        {
            return Err(EncodeError::Failed);
        }
        let size = usize::try_from((*r.pkt).size).map_err(|_| EncodeError::Failed)?;
        Ok(std::slice::from_raw_parts((*r.pkt).data, size).to_vec())
    }
}

/// Bild als PNG nach `file`; legt den Ordner an.
pub fn save_screenshot(f: &Frame, file: &Path) -> bool {
    let ok = match encode_png(f) {
        Ok(png) => {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(file, png).is_ok()
        }
        Err(EncodeError::GpuUnreadable) => return false,
        Err(EncodeError::Failed) => false,
    };
    if !ok {
        log_msg!(LogLevel::Error, "screenshot: Speichern fehlgeschlagen");
    }
    ok
}
