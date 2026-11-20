//! MP4-Aufnahme in eigenem Thread: Video ohne Neukodierung (Remux der Annex-B-Pakete),
//! Ton als AAC-LC 44,1 kHz Stereo 160 kbit/s. `push_video` immer aufrufen – auch ohne
//! Aufnahme hält der Recorder die Pakete seit dem letzten Keyframe vor (nur durch
//! `gop_max_bytes` begrenzt), damit eine Aufnahme mitten im Stream sauber mit einem Keyframe
//! beginnt. Ändert sich die Auflösung, wird die Datei abgeschlossen und
//! "<stem>_teil<n>.mp4" begonnen.

use std::collections::VecDeque;
use std::ffi::{CString, c_int, c_void};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;

use crate::app::log::LogLevel;
use crate::ffi::ffmpeg as av;
use crate::log_msg;
use crate::protocol::packets::VideoPacket;
use crate::record::filenames::{
    LocalTime, contains_keyframe, part_file_name, timestamp_name, unique_path,
};

/// Standard-Obergrenze für vorgehaltene Pakete seit dem letzten Keyframe: 256 MiB.
pub const DEFAULT_GOP_MAX_BYTES: usize = 256 * 1024 * 1024;

const AUDIO_RATE: c_int = 44100;
const AUDIO_BIT_RATE: i64 = 160_000;
const VIDEO_TB: av::AVRational = av::AVRational { num: 1, den: 90000 };
const AUDIO_TB: av::AVRational = av::AVRational {
    num: 1,
    den: AUDIO_RATE,
};
/// `AV_NOPTS_VALUE` (Makro, von bindgen nicht übernommen)
const AV_NOPTS_VALUE: i64 = i64::MIN;
/// `MKTAG('h', 'v', 'c', '1')` – für QuickTime/Windows
const TAG_HVC1: u32 = u32::from_le_bytes(*b"hvc1");
/// AAC-Rahmen, falls der Encoder keine Größe meldet
const DEFAULT_FRAME_SIZE: c_int = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoFormat {
    pub width: i32,
    pub height: i32,
    pub h265: bool,
}

// ------------------------------------------------------------------ Bitstrom-Helfer

/// Startcode-Positionen: (Beginn inkl. Startcode, Kopf-Byte) – wie v1 `parameterSets`.
fn nal_units(d: &[u8]) -> Vec<(usize, usize)> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i + 3 < d.len() {
        if d[i] == 0 && d[i + 1] == 0 {
            if d[i + 2] == 1 {
                nals.push((i, i + 3));
                i += 2;
            } else if d[i + 2] == 0 && d[i + 3] == 1 {
                nals.push((i, i + 4));
                i += 3;
            }
        }
        i += 1;
    }
    nals
}

/// Parametersätze (H.264: SPS/PPS, H.265: VPS/SPS/PPS) eines Annex-B-Pakets, inkl. Startcodes.
fn parameter_sets(p: &VideoPacket) -> Vec<u8> {
    let d = &p.data;
    let nals = nal_units(d);
    let mut out = Vec::new();
    for (n, &(start, hdr)) in nals.iter().enumerate() {
        let end = nals.get(n + 1).map_or(d.len(), |next| next.0);
        let Some(&h) = d.get(hdr) else { continue };
        let ps = if p.h265 {
            (32..=34).contains(&((h >> 1) & 0x3f))
        } else {
            matches!(h & 0x1f, 7 | 8)
        };
        if ps {
            out.extend_from_slice(&d[start..end]);
        }
    }
    out
}

fn codec_id(h265: bool) -> av::AVCodecID {
    if h265 {
        av::AV_CODEC_ID_HEVC
    } else {
        av::AV_CODEC_ID_H264
    }
}

/// Maße aus dem Bitstrom lesen (unabhängig von Ereignissen).
fn parse_dimensions(key: &VideoPacket) -> Option<(i32, i32)> {
    let len = c_int::try_from(key.data.len()).ok()?;
    let mut buf = key.data.clone();
    buf.resize(buf.len() + av::AV_INPUT_BUFFER_PADDING_SIZE as usize, 0);
    // SAFETY: Parser und Kontext werden hier angelegt und freigegeben; buf ist len Bytes plus
    // die von FFmpeg verlangte Auffüllung groß.
    unsafe {
        let parser = av::av_parser_init(codec_id(key.h265));
        if parser.is_null() {
            return None;
        }
        let mut pc = av::avcodec_alloc_context3(std::ptr::null());
        let mut out_data: *mut u8 = null_mut();
        let mut out_size: c_int = 0;
        av::av_parser_parse2(
            parser,
            pc,
            &mut out_data,
            &mut out_size,
            buf.as_ptr(),
            len,
            AV_NOPTS_VALUE,
            AV_NOPTS_VALUE,
            0,
        );
        let dims = ((*parser).width, (*parser).height);
        av::avcodec_free_context(&mut pc);
        av::av_parser_close(parser);
        (dims.0 > 0 && dims.1 > 0).then_some(dims)
    }
}

// ------------------------------------------------------------------ MP4-Datei (FFI-Hülle)

/// Geöffnete MP4 mit Video- und AAC-Strom; `Drop` gibt alles frei (ohne Trailer).
struct OutFile {
    oc: *mut av::AVFormatContext,
    vs: *mut av::AVStream,
    ast: *mut av::AVStream,
    aenc: *mut av::AVCodecContext,
    fifo: *mut av::AVAudioFifo,
    header_written: bool,
}

impl Drop for OutFile {
    fn drop(&mut self) {
        // SAFETY: Alle Zeiger gehören allein dieser Hülle und sind null oder gültig;
        // die Freigabefunktionen vertragen null bzw. setzen auf null.
        unsafe {
            if !self.oc.is_null() {
                if !(*self.oc).pb.is_null() {
                    av::avio_closep(&mut (*self.oc).pb);
                }
                av::avformat_free_context(self.oc);
                self.oc = null_mut();
            }
            av::avcodec_free_context(&mut self.aenc);
            if !self.fifo.is_null() {
                av::av_audio_fifo_free(self.fifo);
                self.fifo = null_mut();
            }
        }
    }
}

/// Leeres Ton-Frame (FLTP, Stereo, 44,1 kHz) mit `n` Samples.
fn audio_frame(n: c_int) -> *mut av::AVFrame {
    // SAFETY: Frame wird hier angelegt; bei Fehler freigegeben.
    unsafe {
        let mut f = av::av_frame_alloc();
        if f.is_null() {
            return f;
        }
        (*f).nb_samples = n;
        (*f).format = av::AV_SAMPLE_FMT_FLTP;
        (*f).sample_rate = AUDIO_RATE;
        av::av_channel_layout_default(&mut (*f).ch_layout, 2);
        if av::av_frame_get_buffer(f, 0) < 0 {
            av::av_frame_free(&mut f);
        }
        f
    }
}

impl OutFile {
    fn create(
        path_utf8: &str,
        h265: bool,
        width: i32,
        height: i32,
        params: &[u8],
    ) -> Result<OutFile, String> {
        let cpath = CString::new(path_utf8)
            .map_err(|_| format!("Datei kann nicht geschrieben werden: {path_utf8}"))?;
        let mut f = OutFile {
            oc: null_mut(),
            vs: null_mut(),
            ast: null_mut(),
            aenc: null_mut(),
            fifo: null_mut(),
            header_written: false,
        };
        // SAFETY: Alle Objekte werden hier angelegt und vor Benutzung geprüft; bei Fehlern
        // gibt `Drop` sie frei. extradata gehört danach dem Stream (FFmpeg gibt sie frei).
        unsafe {
            if av::avformat_alloc_output_context2(
                &mut f.oc,
                std::ptr::null(),
                c"mp4".as_ptr(),
                cpath.as_ptr(),
            ) < 0
                || f.oc.is_null()
            {
                return Err("MP4 kann nicht angelegt werden".into());
            }
            f.vs = av::avformat_new_stream(f.oc, std::ptr::null());
            if f.vs.is_null() {
                return Err("MP4 kann nicht angelegt werden".into());
            }
            (*f.vs).time_base = VIDEO_TB;
            let par = &mut *(*f.vs).codecpar;
            par.codec_type = av::AVMEDIA_TYPE_VIDEO;
            par.codec_id = codec_id(h265);
            par.width = width;
            par.height = height;
            par.extradata =
                av::av_mallocz(params.len() + av::AV_INPUT_BUFFER_PADDING_SIZE as usize)
                    .cast::<u8>();
            if par.extradata.is_null() {
                return Err("MP4 kann nicht angelegt werden".into());
            }
            std::ptr::copy_nonoverlapping(params.as_ptr(), par.extradata, params.len());
            par.extradata_size = params.len() as c_int;
            if h265 {
                par.codec_tag = TAG_HVC1;
            }

            let aac = av::avcodec_find_encoder(av::AV_CODEC_ID_AAC);
            if !aac.is_null() {
                f.aenc = av::avcodec_alloc_context3(aac);
            }
            if f.aenc.is_null() {
                return Err("AAC-Encoder öffnet nicht".into());
            }
            let enc = &mut *f.aenc;
            enc.sample_rate = AUDIO_RATE;
            enc.sample_fmt = av::AV_SAMPLE_FMT_FLTP;
            enc.bit_rate = AUDIO_BIT_RATE;
            av::av_channel_layout_default(&mut enc.ch_layout, 2);
            enc.time_base = AUDIO_TB;
            if (*(*f.oc).oformat).flags & av::AVFMT_GLOBALHEADER as c_int != 0 {
                enc.flags |= av::AV_CODEC_FLAG_GLOBAL_HEADER as c_int;
            }
            if av::avcodec_open2(f.aenc, aac, null_mut()) < 0 {
                return Err("AAC-Encoder öffnet nicht".into());
            }
            f.ast = av::avformat_new_stream(f.oc, std::ptr::null());
            if f.ast.is_null() {
                return Err("MP4 kann nicht angelegt werden".into());
            }
            (*f.ast).time_base = AUDIO_TB;
            av::avcodec_parameters_from_context((*f.ast).codecpar, f.aenc);
            f.fifo = av::av_audio_fifo_alloc(av::AV_SAMPLE_FMT_FLTP, 2, 4096);
            if f.fifo.is_null() {
                return Err("AAC-Encoder öffnet nicht".into());
            }

            // FFmpeg nimmt unter Windows UTF-8-Pfade an (Umlaute im Benutzerordner).
            if av::avio_open(
                &mut (*f.oc).pb,
                cpath.as_ptr(),
                av::AVIO_FLAG_WRITE as c_int,
            ) < 0
            {
                return Err(format!("Datei kann nicht geschrieben werden: {path_utf8}"));
            }
            let mut opts: *mut av::AVDictionary = null_mut();
            av::av_dict_set(&mut opts, c"movflags".as_ptr(), c"+faststart".as_ptr(), 0);
            let rc = av::avformat_write_header(f.oc, &mut opts);
            av::av_dict_free(&mut opts);
            if rc < 0 {
                return Err("MP4-Kopf kann nicht geschrieben werden".into());
            }
            f.header_written = true;
        }
        Ok(f)
    }

    /// Paket mit `pts` in 90 kHz schreiben.
    fn write_video(&mut self, data: &[u8], pts: i64, key: bool) {
        let Ok(len) = c_int::try_from(data.len()) else {
            return;
        };
        // SAFETY: oc/vs sind gültig (create); pkt wird hier angelegt und freigegeben.
        unsafe {
            let mut pkt = av::av_packet_alloc();
            if pkt.is_null() {
                return;
            }
            if av::av_new_packet(pkt, len) == 0 {
                std::ptr::copy_nonoverlapping(data.as_ptr(), (*pkt).data, data.len());
                (*pkt).pts = pts;
                (*pkt).dts = pts;
                (*pkt).duration = 0;
                (*pkt).stream_index = (*self.vs).index;
                if key {
                    (*pkt).flags |= av::AV_PKT_FLAG_KEY as c_int;
                }
                av::av_packet_rescale_ts(pkt, VIDEO_TB, (*self.vs).time_base);
                if av::av_interleaved_write_frame(self.oc, pkt) < 0 {
                    log_msg!(LogLevel::Warn, "recorder: Videopaket verworfen");
                }
            }
            av::av_packet_free(&mut pkt);
        }
    }

    fn frame_size(&self) -> c_int {
        // SAFETY: aenc ist geöffnet (create).
        let n = unsafe { (*self.aenc).frame_size };
        if n > 0 { n } else { DEFAULT_FRAME_SIZE }
    }

    fn fifo_size(&self) -> c_int {
        // SAFETY: fifo ist gültig (create).
        unsafe { av::av_audio_fifo_size(self.fifo) }
    }

    /// Planare Samples (links, rechts – gleich lang) anhängen.
    fn fifo_write(&mut self, left: &[f32], right: &[f32]) {
        let n = left.len().min(right.len());
        let Ok(n) = c_int::try_from(n) else { return };
        if n == 0 {
            return;
        }
        let planes = [
            left.as_ptr().cast_mut().cast::<c_void>(),
            right.as_ptr().cast_mut().cast::<c_void>(),
        ];
        // SAFETY: fifo ist gültig; beide Ebenen enthalten mindestens n Samples und werden nur
        // gelesen.
        unsafe { av::av_audio_fifo_write(self.fifo, planes.as_ptr(), n) };
    }

    /// `n` Samples aus dem FIFO kodieren (Zeitstempel `pts` in 1/44100).
    fn encode_from_fifo(&mut self, n: c_int, pts: i64) {
        let mut f = audio_frame(n);
        if f.is_null() {
            return;
        }
        // SAFETY: f hat Platz für n Samples je Ebene; fifo ist gültig.
        unsafe {
            av::av_audio_fifo_read(self.fifo, (*f).data.as_ptr().cast::<*mut c_void>(), n);
            (*f).pts = pts;
            self.encode_audio(f);
            av::av_frame_free(&mut f);
        }
    }

    /// `frame` null = Encoder leeren.
    fn encode_audio(&mut self, frame: *const av::AVFrame) {
        // SAFETY: aenc/oc/ast sind gültig (create); pkt wird hier angelegt und freigegeben.
        unsafe {
            if av::avcodec_send_frame(self.aenc, frame) < 0 {
                return;
            }
            let mut pkt = av::av_packet_alloc();
            if pkt.is_null() {
                return;
            }
            while av::avcodec_receive_packet(self.aenc, pkt) == 0 {
                (*pkt).stream_index = (*self.ast).index;
                av::av_packet_rescale_ts(pkt, AUDIO_TB, (*self.ast).time_base);
                av::av_interleaved_write_frame(self.oc, pkt);
            }
            av::av_packet_free(&mut pkt);
        }
    }

    /// Ton-Rest kodieren, Trailer schreiben (faststart schreibt die Datei dabei um), schließen.
    fn finish(mut self, audio_next_pts: i64) {
        let rest = self.fifo_size();
        if rest > 0 && audio_next_pts >= 0 {
            self.encode_from_fifo(rest, audio_next_pts);
        }
        if audio_next_pts >= 0 {
            self.encode_audio(std::ptr::null());
        }
        if self.header_written {
            // SAFETY: oc ist gültig, der Kopf wurde geschrieben.
            unsafe { av::av_write_trailer(self.oc) };
        }
        // Drop schließt die Datei und gibt alles frei.
    }
}

// ------------------------------------------------------------------ Thread

enum Cmd {
    Video(VideoPacket),
    Audio(Vec<f32>, u64),
    Start(PathBuf, VideoFormat),
    Format(VideoFormat),
    Stop(mpsc::Sender<()>),
    Sync(mpsc::Sender<()>),
}

/// Zustand, den die API liest.
#[derive(Default)]
struct State {
    active: bool,
    files: Vec<PathBuf>,
    current: PathBuf,
    last_error: String,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// recording && waiting_key, für die API
    waiting: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Nur im Schreib-Thread.
struct Worker {
    shared: Arc<Shared>,
    gop_max_bytes: usize,
    gop: VecDeque<VideoPacket>,
    gop_bytes: usize,
    recording: bool,
    waiting_key: bool,
    dir: PathBuf,
    stem: String,
    part: i32,
    file_fmt: VideoFormat,
    file_params: Vec<u8>,
    out: Option<OutFile>,
    t0: u64,
    last_video_pts: Option<i64>,
    audio_next_pts: i64,
}

impl Worker {
    fn new(shared: Arc<Shared>, gop_max_bytes: usize) -> Self {
        Self {
            shared,
            gop_max_bytes,
            gop: VecDeque::new(),
            gop_bytes: 0,
            recording: false,
            waiting_key: false,
            dir: PathBuf::new(),
            stem: String::new(),
            part: 1,
            file_fmt: VideoFormat::default(),
            file_params: Vec::new(),
            out: None,
            t0: 0,
            last_video_pts: None,
            audio_next_pts: -1,
        }
    }

    fn run(mut self, rx: mpsc::Receiver<Cmd>) {
        while let Ok(c) = rx.recv() {
            self.handle(c);
        }
        self.close_file();
    }

    fn fail(&self, msg: &str) {
        log_msg!(LogLevel::Error, "recorder: {}", msg);
        self.shared.lock().last_error = msg.to_string();
    }

    fn handle(&mut self, c: Cmd) {
        let done = match c {
            Cmd::Video(p) => {
                self.on_video(p);
                None
            }
            Cmd::Audio(pcm, ts) => {
                if self.out.is_some() {
                    self.write_audio(&pcm, ts);
                }
                None
            }
            Cmd::Start(dir, fmt) => {
                self.on_start(dir, fmt);
                None
            }
            Cmd::Format(fmt) => {
                if self.recording
                    && self.out.is_some()
                    && fmt.width > 0
                    && (fmt.width != self.file_fmt.width
                        || fmt.height != self.file_fmt.height
                        || fmt.h265 != self.file_fmt.h265)
                {
                    self.close_file();
                    self.part += 1;
                    self.waiting_key = true;
                }
                if fmt.width > 0 {
                    self.file_fmt = fmt;
                }
                None
            }
            Cmd::Stop(done) => {
                self.close_file();
                self.recording = false;
                self.waiting_key = false;
                Some(done)
            }
            Cmd::Sync(done) => Some(done),
        };
        self.shared
            .waiting
            .store(self.recording && self.waiting_key, Ordering::SeqCst);
        if let Some(done) = done {
            let _ = done.send(());
        }
    }

    fn on_start(&mut self, dir: PathBuf, fmt: VideoFormat) {
        self.close_file();
        self.recording = true;
        self.dir = dir;
        self.part = 1;
        self.file_fmt = fmt;
        let first = unique_path(
            &self.dir,
            &timestamp_name("AirPlay", &LocalTime::now(), "mp4"),
        );
        self.stem = first
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.waiting_key = true;
        {
            let mut st = self.shared.lock();
            st.files.clear();
            st.last_error.clear();
        }
        // Läuft der Stream schon: ab dem vorgehaltenen Keyframe schreiben (nötig zum Dekodieren),
        // die Wiedergabe beginnt aber beim neuesten Bild = Zeitpunkt des Tastendrucks. Die Bilder
        // davor bekommen negative Zeitstempel; der MP4-Muxer legt dafür eine Edit-List an.
        let gop = std::mem::take(&mut self.gop);
        if let (Some(front), Some(back)) = (gop.front(), gop.back())
            && contains_keyframe(front)
            && self.open_file(front)
        {
            self.waiting_key = false;
            self.t0 = back.ntp_local;
            for (i, p) in gop.iter().enumerate() {
                self.write_video(p, i == 0);
            }
        }
        self.gop = gop;
    }

    fn cache_gop(&mut self, p: &VideoPacket, key: bool) {
        if key {
            self.gop.clear();
            self.gop_bytes = 0;
        }
        if self.gop.is_empty() && !key {
            return; // ohne Keyframe am Anfang nutzlos
        }
        self.gop.push_back(p.clone());
        self.gop_bytes += p.data.len();
        if self.gop_bytes > self.gop_max_bytes {
            // nur Speichergrenze, keine Zeitgrenze (Keyframes sind selten)
            log_msg!(
                LogLevel::Warn,
                "recorder: Keyframe-Vorrat über {} MB, verworfen",
                self.gop_max_bytes >> 20
            );
            self.gop.clear();
            self.gop_bytes = 0;
        }
    }

    fn on_video(&mut self, p: VideoPacket) {
        let key = contains_keyframe(&p);
        self.cache_gop(&p, key);
        if !self.recording {
            return;
        }
        if key && self.out.is_some() {
            let params = parameter_sets(&p);
            if !params.is_empty() && params != self.file_params {
                // Auflösung/Codec gewechselt (z. B. gedreht)
                self.close_file();
                self.part += 1;
                self.waiting_key = true;
            }
        }
        if self.waiting_key {
            if !key || !self.open_file(&p) {
                return;
            }
            self.waiting_key = false;
        }
        self.write_video(&p, key);
    }

    fn open_file(&mut self, key: &VideoPacket) -> bool {
        self.file_params = parameter_sets(key);
        if self.file_params.is_empty() {
            self.fail("Keyframe ohne Parametersätze");
            return false;
        }
        let (width, height) =
            parse_dimensions(key).unwrap_or((self.file_fmt.width, self.file_fmt.height));
        if width <= 0 || height <= 0 {
            self.fail("Videomaße unbekannt");
            return false;
        }
        self.file_fmt = VideoFormat {
            width,
            height,
            h265: key.h265,
        };

        let _ = std::fs::create_dir_all(&self.dir);
        let path = unique_path(&self.dir, &part_file_name(&self.stem, self.part, "mp4"));
        let Some(path_utf8) = path.to_str().map(str::to_string) else {
            self.fail(&format!(
                "Datei kann nicht geschrieben werden: {}",
                path.display()
            ));
            return false;
        };
        match OutFile::create(&path_utf8, key.h265, width, height, &self.file_params) {
            Ok(out) => self.out = Some(out),
            Err(msg) => {
                self.fail(&msg);
                return false;
            }
        }
        self.t0 = key.ntp_local;
        self.last_video_pts = None;
        self.audio_next_pts = -1;
        {
            let mut st = self.shared.lock();
            st.files.push(path.clone());
            st.current = path;
        }
        log_msg!(
            LogLevel::Info,
            "recorder: schreibe {} ({}x{} {})",
            path_utf8,
            width,
            height,
            if key.h265 { "H.265" } else { "H.264" }
        );
        true
    }

    fn write_video(&mut self, p: &VideoPacket, key: bool) {
        let Some(out) = self.out.as_mut() else {
            return;
        };
        // ns → 90 kHz, vor t0 negativ
        let mut pts = (p.ntp_local as i64).wrapping_sub(self.t0 as i64) * 9 / 100_000;
        if let Some(last) = self.last_video_pts
            && pts <= last
        {
            pts = last + 1;
        }
        self.last_video_pts = Some(pts);
        out.write_video(&p.data, pts, key);
    }

    fn write_audio(&mut self, pcm: &[f32], ts: u64) {
        let Some(out) = self.out.as_mut() else {
            return;
        };
        let rate = AUDIO_RATE as u64;
        let frames = pcm.len() / 2;
        let t0 = self.t0;
        let mut skip = 0usize;
        if self.audio_next_pts < 0 {
            if ts + frames as u64 * 1_000_000_000 / rate <= t0 {
                return; // komplett vor Dateibeginn
            }
            if ts < t0 {
                skip = ((t0 - ts) * rate / 1_000_000_000) as usize;
                self.audio_next_pts = 0;
            } else {
                self.audio_next_pts = ((ts - t0) * rate / 1_000_000_000) as i64;
            }
        } else {
            // Wo liegt dieser Block laut Zeitstempel, wo stünde er ohne Lücke?
            let block_pts = (ts as i64).wrapping_sub(t0 as i64) * AUDIO_RATE as i64 / 1_000_000_000;
            let expected = self.audio_next_pts + i64::from(out.fifo_size());
            let diff = block_pts - expected;
            let frame_size = i64::from(out.frame_size());
            if diff > frame_size {
                // Lücke (z. B. Funkpause) → Stille einfügen
                let silence = vec![0f32; diff as usize];
                out.fifo_write(&silence, &silence);
            } else if diff < -frame_size {
                // Überlappung → doppelten Anfang überspringen
                skip = (-diff).min(frames as i64) as usize;
            }
        }
        if skip >= frames {
            return;
        }
        let (left, right): (Vec<f32>, Vec<f32>) = pcm[skip * 2..frames * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[l, r]| (l, r))
            .unzip();
        out.fifo_write(&left, &right);
        let frame_size = out.frame_size();
        while out.fifo_size() >= frame_size {
            out.encode_from_fifo(frame_size, self.audio_next_pts);
            self.audio_next_pts += i64::from(frame_size);
        }
    }

    fn close_file(&mut self) {
        let Some(out) = self.out.take() else {
            return;
        };
        out.finish(self.audio_next_pts);
        let done = std::mem::take(&mut self.shared.lock().current);
        log_msg!(LogLevel::Info, "recorder: abgeschlossen {}", done.display());
    }
}

// ------------------------------------------------------------------ API

/// Aufnahme-Steuerung; alle Methoden nehmen `&self` (App-Thread, Ton-Thread und der
/// Abschluss-Thread teilen sich den Recorder über `Arc`).
pub struct Recorder {
    tx: Option<mpsc::Sender<Cmd>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new(DEFAULT_GOP_MAX_BYTES)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop();
        self.tx = None; // Thread endet, sobald die Warteschlange leer ist
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Recorder {
    /// `gop_max_bytes`: Obergrenze für vorgehaltene Pakete seit dem letzten Keyframe (keine
    /// Zeitgrenze).
    pub fn new(gop_max_bytes: usize) -> Self {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel();
        let for_thread = shared.clone();
        // Der Worker (mit den FFmpeg-Zeigern) entsteht erst im Thread und verlässt ihn nie.
        let thread = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || Worker::new(for_thread, gop_max_bytes).run(rx));
        match thread {
            Ok(t) => Self {
                tx: Some(tx),
                thread: Some(t),
                shared,
            },
            Err(e) => {
                log_msg!(LogLevel::Error, "recorder: Thread startet nicht: {}", e);
                Self {
                    tx: None,
                    thread: None,
                    shared,
                }
            }
        }
    }

    fn send(&self, c: Cmd) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(c).is_ok())
    }

    /// Schickt einen Befehl mit Rückmeldung und wartet darauf.
    fn send_and_wait(&self, make: impl FnOnce(mpsc::Sender<()>) -> Cmd) {
        let (done_tx, done_rx) = mpsc::channel();
        if self.send(make(done_tx)) {
            // Err nur, wenn der Thread nicht mehr läuft – dann gibt es nichts zu warten.
            let _ = done_rx.recv();
        }
    }

    pub fn start(&self, dir: &Path, format: VideoFormat) -> bool {
        let _ = std::fs::create_dir_all(dir);
        if !dir.is_dir() {
            self.shared.lock().last_error = format!("Ordner nicht verfügbar: {}", dir.display());
            return false;
        }
        self.shared.lock().active = true;
        if !self.send(Cmd::Start(dir.to_path_buf(), format)) {
            let mut st = self.shared.lock();
            st.active = false;
            st.last_error = "Schreib-Thread läuft nicht".into();
            return false;
        }
        true
    }

    pub fn push_video(&self, packet: &VideoPacket) {
        self.send(Cmd::Video(packet.clone()));
    }

    /// Interleaved Stereo (`pcm.len() / 2` Frames), Zeitstempel des ersten Frames in ns.
    pub fn push_audio(&self, pcm: &[f32], ts_ns: u64) {
        let frames = pcm.len() / 2;
        if !self.active() || frames == 0 {
            return;
        }
        self.send(Cmd::Audio(pcm[..frames * 2].to_vec(), ts_ns));
    }

    pub fn on_format_change(&self, format: VideoFormat) {
        self.send(Cmd::Format(format));
    }

    /// Schließt sauber ab, wartet auf den Schreib-Thread; idempotent.
    pub fn stop(&self) {
        self.send_and_wait(Cmd::Stop);
        self.shared.lock().active = false;
    }

    pub fn active(&self) -> bool {
        self.shared.lock().active
    }

    pub fn current_file(&self) -> PathBuf {
        self.shared.lock().current.clone()
    }

    /// Dateien der laufenden/letzten Aufnahme
    pub fn files(&self) -> Vec<PathBuf> {
        self.shared.lock().files.clone()
    }

    /// Zuletzt aufgetretener Fehler (leer = keiner), z. B. für eine Einblendung.
    pub fn last_error(&self) -> String {
        self.shared.lock().last_error.clone()
    }

    /// Aufnahme läuft, aber es fehlt noch ein Keyframe (Vorrat war übergelaufen) – nichts wird
    /// geschrieben.
    pub fn waiting_for_keyframe(&self) -> bool {
        self.shared.waiting.load(Ordering::SeqCst)
    }

    /// Wartet, bis alle bisher übergebenen Befehle verarbeitet sind (für Tests/Statusabfragen).
    pub fn sync(&self) {
        self.send_and_wait(Cmd::Sync);
    }
}
