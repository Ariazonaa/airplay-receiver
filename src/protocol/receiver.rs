//! Hülle um UxPlays raop: startet Server + Ankündigung wie uxplay.cpp v1.73.7, aber ohne
//! GStreamer. Die Callbacks schieben nur in Queues (blockieren nie, paniken nie).
//! Es darf nur eine Instanz gleichzeitig laufen (die DNS-SD-Schnittstelle ist global).
//!
//! Der Callback-Kontext (`Shared`) liegt in einem `Arc`, den der `Receiver` von `new()` bis
//! zu seinem Drop hält. `Drop` ruft erst `stop()` (also `raop_destroy`, das die UxPlay-Threads
//! beendet) und gibt danach den Kontext frei.

use std::ffi::{CStr, CString, c_char, c_int, c_uchar, c_ushort, c_void};
use std::os::windows::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU8, Ordering};

use windows::Win32::Storage::FileSystem::GetShortPathNameW;
use windows::core::PCWSTR;

use crate::app::log::{LogLevel, log_raop_bridge};
use crate::discovery::adapters::NetAdapter;
use crate::discovery::dnssd::{
    dnssd_destroy, dnssd_init, dnssd_register_airplay, dnssd_register_raop,
    dnssd_set_airplay_features, dnssd_unregister_airplay, dnssd_unregister_raop,
    dnssd_win_set_interface, dnssd_win_set_log,
};
use crate::ffi::uxplay::{
    self as ux, audio_decode_struct, dnssd_t, playback_info_t, raop_callbacks_t, raop_ntp_t,
    raop_t, reset_type_t, video_codec_t, video_decode_struct,
};
use crate::log_msg;
use crate::platform::win::paths::{KnownDir, from_wide, known_dir};
use crate::protocol::packet_queue::PacketQueue;
use crate::protocol::packets::{AudioPacket, SessionEvent, VideoPacket};

/// Sekunden, wie UxPlay MISSED_FEEDBACK_LIMIT
const MISSED_FEEDBACK_LIMIT: i32 = 15;
const SECOND_NS: u64 = 1_000_000_000;
/// 30 s bei 60 fps: komprimierte Pakete dürfen nicht verloren gehen (selten Keyframes).
const VIDEO_CAPACITY: usize = 1800;
const AUDIO_CAPACITY: usize = 400;
const EVENT_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReceiverError {
    #[default]
    None,
    DnssdInit,
    RaopInit,
    KeyFile,
    PortInUse,
    AnnounceFailed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceiverOptions {
    pub name: String,
    pub adapter: NetAdapter,
    pub allow_h265: bool,
    pub verbose_log: bool,
}

/// Alles, was die UxPlay-Callbacks anfassen. `cls` zeigt auf dieses Objekt.
struct Shared {
    video: Arc<PacketQueue<VideoPacket>>,
    audio: Arc<PacketQueue<AudioPacket>>,
    events: Arc<PacketQueue<SessionEvent>>,
    open_connections: AtomicI32,
    missed_feedback: AtomicI32,
    reset_pending: AtomicBool,
    clock_offset: AtomicI64,
    /// aus audio_get_format; Pakete tragen oft ct=0
    audio_ct: AtomicU8,
}

/// Kontext der laufenden Instanz (nur eine gleichzeitig).
static ACTIVE: AtomicPtr<Shared> = AtomicPtr::new(null_mut());

/// Wie `Receiver::now_ns()`, ohne Empfänger (z. B. als Uhr für den Ton-Thread).
pub fn local_time() -> u64 {
    // SAFETY: get_local_time (raop.c) hat keine Vorbedingungen.
    unsafe { ux::get_local_time() }
}

impl Shared {
    /// Wie uxplay.cpp: beim ersten Paket einer Sitzung Versatz Remote→Lokal festlegen.
    fn to_local(&self, ntp_remote: u64, ntp_local: u64) -> u64 {
        let mut offset = self.clock_offset.load(Ordering::SeqCst);
        if offset == 0 {
            let local = if ntp_local != 0 {
                ntp_local
            } else {
                local_time()
            };
            offset = local.wrapping_sub(ntp_remote) as i64;
            if offset == 0 {
                offset = 1;
            }
            self.clock_offset.store(offset, Ordering::SeqCst);
        }
        (ntp_remote as i64).wrapping_add(offset) as u64
    }
}

pub struct Receiver {
    options: ReceiverOptions,
    error: ReceiverError,
    raop: *mut raop_t,
    dnssd: *mut dnssd_t,
    port: u16,
    shared: Arc<Shared>,
    last_watchdog_tick: u64,
}

impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}

impl Receiver {
    /// Legt nur die Queues an; startet nichts.
    pub fn new() -> Self {
        Self {
            options: ReceiverOptions::default(),
            error: ReceiverError::None,
            raop: null_mut(),
            dnssd: null_mut(),
            port: 0,
            shared: Arc::new(Shared {
                video: Arc::new(PacketQueue::new(VIDEO_CAPACITY)),
                audio: Arc::new(PacketQueue::new(AUDIO_CAPACITY)),
                events: Arc::new(PacketQueue::new(EVENT_CAPACITY)),
                open_connections: AtomicI32::new(0),
                missed_feedback: AtomicI32::new(0),
                reset_pending: AtomicBool::new(false),
                clock_offset: AtomicI64::new(0),
                audio_ct: AtomicU8::new(0),
            }),
            last_watchdog_tick: 0,
        }
    }

    pub fn running(&self) -> bool {
        !self.raop.is_null()
    }

    pub fn video(&self) -> &Arc<PacketQueue<VideoPacket>> {
        &self.shared.video
    }

    pub fn audio(&self) -> &Arc<PacketQueue<AudioPacket>> {
        &self.shared.audio
    }

    pub fn events(&self) -> &Arc<PacketQueue<SessionEvent>> {
        &self.shared.events
    }

    /// Zeitbasis von `VideoPacket`/`AudioPacket::ntp_local`: CLOCK_REALTIME in Nanosekunden.
    pub fn now_ns(&self) -> u64 {
        local_time()
    }

    pub fn open_connections(&self) -> i32 {
        self.shared.open_connections.load(Ordering::SeqCst)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn last_error(&self) -> ReceiverError {
        self.error
    }

    pub fn adapter(&self) -> &NetAdapter {
        &self.options.adapter
    }

    fn shared_ptr(&self) -> *mut Shared {
        Arc::as_ptr(&self.shared) as *mut Shared
    }

    pub fn start(&mut self, o: &ReceiverOptions) -> bool {
        if self.running() {
            self.stop();
        }
        let me = self.shared_ptr();
        if let Err(other) =
            ACTIVE.compare_exchange(null_mut(), me, Ordering::SeqCst, Ordering::SeqCst)
            && other != me
        {
            log_msg!(
                LogLevel::Error,
                "receiver: es läuft bereits eine andere Instanz"
            );
            return false;
        }
        self.options = o.clone();
        self.error = ReceiverError::None;
        let s = &self.shared;
        s.video.clear();
        s.audio.clear();
        s.events.clear();
        s.open_connections.store(0, Ordering::SeqCst);
        s.missed_feedback.store(0, Ordering::SeqCst);
        s.reset_pending.store(false, Ordering::SeqCst);
        s.clock_offset.store(0, Ordering::SeqCst);

        let ip = o.adapter.ipv4.first().cloned().unwrap_or_default();
        let ip_c = CString::new(ip.clone()).unwrap_or_default();
        dnssd_win_set_log(Some(dnssd_log_bridge));
        // SAFETY: ip_c ist nullterminiert und lebt während des Aufrufs.
        unsafe { dnssd_win_set_interface(o.adapter.if_index, ip_c.as_ptr()) };

        let mut err: c_int = 0;
        // SAFETY: Name und MAC zeigen auf die angegebene Anzahl Bytes; err ist beschreibbar.
        self.dnssd = unsafe {
            dnssd_init(
                o.name.as_ptr() as *const c_char,
                o.name.len() as c_int,
                o.adapter.mac.as_ptr() as *const c_char,
                o.adapter.mac.len() as c_int,
                &mut err,
                0,
            )
        };
        if self.dnssd.is_null() {
            log_msg!(LogLevel::Error, "receiver: dnssd_init Fehler {}", err);
            self.stop();
            self.error = ReceiverError::DnssdInit;
            return false;
        }
        set_features(self.dnssd, o.allow_h265);

        let mut cbs = callbacks(me as *mut c_void);
        // SAFETY: raop_init kopiert die Callback-Struktur; cls bleibt bis nach raop_destroy gültig.
        self.raop = unsafe { ux::raop_init(&mut cbs) };
        if self.raop.is_null() {
            log_msg!(LogLevel::Error, "receiver: raop_init fehlgeschlagen");
            self.stop();
            self.error = ReceiverError::RaopInit;
            return false;
        }
        let level = if o.verbose_log {
            ux::LOGGER_DEBUG
        } else {
            ux::LOGGER_INFO
        };
        // SAFETY: raop ist gültig; log_raop_bridge passt zu raop_log_callback_t.
        unsafe {
            ux::raop_set_log_callback(self.raop, Some(log_raop_bridge), null_mut());
            ux::raop_set_log_level(self.raop, level as c_int);
        }

        let mac = CString::new(mac_string(&o.adapter.mac)).unwrap_or_default();
        let keyfile = keyfile_path();
        let keyfile_c = CString::new(keyfile.clone()).unwrap_or_default();
        // SAFETY: raop gültig, beide Zeichenketten nullterminiert.
        if unsafe { ux::raop_init2(self.raop, 1, mac.as_ptr(), keyfile_c.as_ptr()) } != 0 {
            log_msg!(
                LogLevel::Error,
                "receiver: raop_init2 fehlgeschlagen (Schlüssel {})",
                keyfile
            );
            self.stop();
            self.error = ReceiverError::KeyFile;
            return false;
        }

        // Anzeige, die dem iPhone gemeldet wird (UxPlay-Standard, aber 60 statt 30 fps).
        // SAFETY: raop gültig, Schlüssel nullterminiert.
        unsafe {
            ux::raop_set_plist(
                self.raop,
                c"width".as_ptr(),
                if o.allow_h265 { 3840 } else { 1920 },
            );
            ux::raop_set_plist(
                self.raop,
                c"height".as_ptr(),
                if o.allow_h265 { 2160 } else { 1080 },
            );
            ux::raop_set_plist(self.raop, c"refreshRate".as_ptr(), 60);
            ux::raop_set_plist(self.raop, c"maxFPS".as_ptr(), 60);
        }

        // Feste Ports wie "uxplay -p": TCP 7100/7000/7001, UDP 7011/6001/6000.
        let mut tcp: [c_ushort; 3] = [7100, 7000, 7001];
        let mut udp: [c_ushort; 3] = [7011, 6001, 6000];
        // SAFETY: raop gültig; die Ports werden kopiert.
        let started = unsafe {
            ux::raop_set_tcp_ports(self.raop, tcp.as_mut_ptr());
            ux::raop_set_udp_ports(self.raop, udp.as_mut_ptr());
            self.port = ux::raop_get_port(self.raop);
            ux::raop_start_httpd(self.raop, &mut self.port)
        };
        if started < 0 {
            log_msg!(
                LogLevel::Error,
                "receiver: HTTP-Server startet nicht auf Port {} (belegt?)",
                self.port
            );
            self.stop();
            self.error = ReceiverError::PortInUse;
            return false;
        }
        // SAFETY: raop und dnssd gültig.
        let (rc_raop, rc_airplay) = unsafe {
            ux::raop_set_port(self.raop, self.port);
            ux::raop_set_dnssd(self.raop, self.dnssd);
            (
                dnssd_register_raop(self.dnssd, self.port),
                dnssd_register_airplay(self.dnssd, self.port),
            )
        };
        log_msg!(
            LogLevel::Info,
            "receiver: gestartet als \"{}\" auf {}:{} (raop={} airplay={}, H.265 {})",
            o.name,
            if ip.is_empty() { "?" } else { ip.as_str() },
            self.port,
            rc_raop,
            rc_airplay,
            if o.allow_h265 { "an" } else { "aus" }
        );
        if rc_raop != 0 || rc_airplay != 0 {
            self.stop();
            self.error = ReceiverError::AnnounceFailed;
            return false;
        }
        self.last_watchdog_tick = self.now_ns();
        true
    }

    pub fn stop(&mut self) {
        // SAFETY: die Zeiger stammen aus dnssd_init/raop_init und werden danach genullt.
        unsafe {
            if !self.dnssd.is_null() {
                dnssd_unregister_raop(self.dnssd);
                dnssd_unregister_airplay(self.dnssd);
            }
            if !self.raop.is_null() {
                // Beendet die UxPlay-Threads; danach ruft niemand mehr Callbacks.
                ux::raop_destroy(self.raop);
                self.raop = null_mut();
            }
            if !self.dnssd.is_null() {
                dnssd_destroy(self.dnssd);
                self.dnssd = null_mut();
            }
        }
        self.shared.open_connections.store(0, Ordering::SeqCst);
        let me = self.shared_ptr();
        let _ = ACTIVE.compare_exchange(me, null_mut(), Ordering::SeqCst, Ordering::SeqCst);
    }

    fn restart_httpd(&mut self) {
        log_msg!(LogLevel::Info, "receiver: setze Verbindungen zurück");
        // SAFETY: nur aus poll() mit gültigem raop.
        unsafe {
            ux::raop_stop_httpd(self.raop);
            ux::raop_remove_known_connections(self.raop);
            let mut p = ux::raop_get_port(self.raop);
            ux::raop_start_httpd(self.raop, &mut p);
            ux::raop_set_port(self.raop, p);
        }
        self.shared.clock_offset.store(0, Ordering::SeqCst);
        self.shared.missed_feedback.store(0, Ordering::SeqCst);
    }

    /// Regelmäßig aus dem Haupt-Thread aufrufen: Heartbeat-Überwachung (15 s wie UxPlay) und
    /// Neustart des HTTP-Servers nach einem Verbindungsabbruch.
    pub fn poll(&mut self) {
        if !self.running() {
            return;
        }
        let now = self.now_ns();
        if now.wrapping_sub(self.last_watchdog_tick) >= SECOND_NS {
            self.last_watchdog_tick = now;
            let s = &self.shared;
            if s.open_connections.load(Ordering::SeqCst) > 0 {
                let missed = s.missed_feedback.fetch_add(1, Ordering::SeqCst) + 1;
                if missed > MISSED_FEEDBACK_LIMIT {
                    log_msg!(
                        LogLevel::Warn,
                        "receiver: seit {} s kein Lebenszeichen vom Gerät",
                        missed
                    );
                    s.reset_pending.store(true, Ordering::SeqCst);
                    s.events.push(SessionEvent::Reset);
                }
            } else {
                s.missed_feedback.store(0, Ordering::SeqCst);
            }
        }
        if self.shared.reset_pending.swap(false, Ordering::SeqCst) {
            self.restart_httpd();
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        // Erst raop_destroy (in stop), danach wird `shared` freigegeben.
        self.stop();
    }
}

extern "C" fn dnssd_log_bridge(level: c_int, msg: *const c_char) {
    // SAFETY: msg kommt aus dnssd.rs und ist nullterminiert; log_raop_bridge panikt nie.
    unsafe { log_raop_bridge(null_mut(), level, msg) }
}

fn mac_string(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// raop/OpenSSL öffnen den Schlüssel mit fopen (ANSI) – daher kurzer 8.3-Pfad.
fn keyfile_path() -> String {
    let dir = known_dir(KnownDir::LocalAppData)
        .unwrap_or_default()
        .join("airplay");
    let _ = std::fs::create_dir_all(&dir);
    let long: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; 260];
    // SAFETY: long ist nullterminiert, buf beschreibbar.
    let mut n = unsafe { GetShortPathNameW(PCWSTR(long.as_ptr()), Some(&mut buf)) } as usize;
    if n > buf.len() {
        buf.resize(n, 0);
        // SAFETY: wie oben, Puffer jetzt groß genug.
        n = unsafe { GetShortPathNameW(PCWSTR(long.as_ptr()), Some(&mut buf)) } as usize;
    }
    let short = if n == 0 || n > buf.len() {
        dir.to_string_lossy().into_owned()
    } else {
        from_wide(&buf[..n]) // 8.3-Pfade sind ASCII
    };
    format!("{short}\\airplay.pem")
}

/// Feature-Bits wie start_dnssd() in uxplay.cpp v1.73.7 (ohne HLS, ohne Legacy-Pairing).
fn set_features(d: *mut dnssd_t, allow_h265: bool) {
    const BITS: [c_int; 32] = [
        0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 1, 0, 1, 1, 0,
        1, 0,
    ];
    // SAFETY: d stammt aus dnssd_init.
    unsafe {
        for (i, &b) in BITS.iter().enumerate() {
            dnssd_set_airplay_features(d, i as c_int, b);
        }
        dnssd_set_airplay_features(d, 0, 0); // HLS aus
        dnssd_set_airplay_features(d, 4, 0); // HLS aus
        dnssd_set_airplay_features(d, 42, c_int::from(allow_h265));
        dnssd_set_airplay_features(d, 27, 0); // kein Legacy-Pairing (UxPlay-Standard)
    }
}

// ---------------------------------------------------------------------------------------------
// UxPlay-Callbacks. Sie laufen in Threads des Kerns: nur in Queues schieben, nie blockieren,
// nie paniken (Rumpf in catch_unwind).

/// Führt `f` mit dem Kontext aus `cls` aus; bei Panik wird geloggt und `fallback` geliefert.
fn with_shared<R>(cls: *mut c_void, fallback: R, f: impl FnOnce(&Shared) -> R) -> R {
    if cls.is_null() {
        return fallback;
    }
    // SAFETY: cls ist Arc::as_ptr(&Receiver::shared); der Receiver hält das Arc, bis
    // raop_destroy zurückgekehrt ist und kein UxPlay-Thread mehr läuft.
    let shared = unsafe { &*(cls as *const Shared) };
    match catch_unwind(AssertUnwindSafe(|| f(shared))) {
        Ok(r) => r,
        Err(_) => {
            let _ = catch_unwind(|| {
                log_msg!(
                    LogLevel::Error,
                    "receiver: Panik in einem UxPlay-Callback abgefangen"
                )
            });
            fallback
        }
    }
}

/// C-Zeichenkette (UTF-8 vom Gerät) oder `None`.
fn c_string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: UxPlay übergibt nullterminierte Zeichenketten.
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn copy_bytes(p: *const c_uchar, len: c_int) -> Vec<u8> {
    if p.is_null() || len <= 0 {
        return Vec::new();
    }
    // SAFETY: UxPlay liefert `len` lesbare Bytes ab `p`.
    unsafe { std::slice::from_raw_parts(p, len as usize) }.to_vec()
}

/// Liest `*p` oder liefert den Standardwert, wenn `p` null ist.
fn read_or<T: Copy>(p: *const T, default: T) -> T {
    if p.is_null() {
        default
    } else {
        // SAFETY: nicht null; UxPlay übergibt gültige Zeiger.
        unsafe { *p }
    }
}

unsafe extern "C" fn cb_conn_init(cls: *mut c_void) {
    with_shared(cls, (), |s| {
        let n = s.open_connections.fetch_add(1, Ordering::SeqCst) + 1;
        log_msg!(LogLevel::Debug, "receiver: Verbindungen offen: {}", n);
    })
}

unsafe extern "C" fn cb_conn_destroy(cls: *mut c_void) {
    with_shared(cls, (), |s| {
        let n = s.open_connections.fetch_sub(1, Ordering::SeqCst) - 1;
        log_msg!(LogLevel::Debug, "receiver: Verbindungen offen: {}", n);
        if n <= 0 {
            s.open_connections.store(0, Ordering::SeqCst);
            s.clock_offset.store(0, Ordering::SeqCst);
            s.events.push(SessionEvent::Disconnected);
        }
    })
}

unsafe extern "C" fn cb_conn_feedback(cls: *mut c_void) {
    with_shared(cls, (), |s| s.missed_feedback.store(0, Ordering::SeqCst))
}

unsafe extern "C" fn cb_conn_reset(cls: *mut c_void, reason: c_int) {
    with_shared(cls, (), |s| {
        log_msg!(
            LogLevel::Warn,
            "receiver: Verbindung verloren (Grund {})",
            reason
        );
        s.reset_pending.store(true, Ordering::SeqCst);
        s.events.push(SessionEvent::Reset);
    })
}

unsafe extern "C" fn cb_video_reset(cls: *mut c_void, ty: reset_type_t) {
    with_shared(cls, (), |s| {
        log_msg!(LogLevel::Info, "receiver: video_reset Typ {}", ty);
        s.clock_offset.store(0, Ordering::SeqCst);
        match ty {
            ux::RESET_TYPE_RTP_SHUTDOWN
            | ux::RESET_TYPE_RTP_TO_HLS_TEARDOWN
            | ux::RESET_TYPE_HLS_SHUTDOWN => s.events.push(SessionEvent::Disconnected),
            // NOHOLD: neues Gerät übernimmt, Connected folgt über report_client_request
            _ => {}
        }
    })
}

unsafe extern "C" fn cb_report_client_request(
    cls: *mut c_void,
    deviceid: *mut c_char,
    model: *mut c_char,
    name: *mut c_char,
    admit: *mut bool,
) {
    with_shared(cls, (), |s| {
        if !admit.is_null() {
            // SAFETY: nicht null, von UxPlay beschreibbar übergeben.
            unsafe { *admit = true };
        }
        let name = c_string(name);
        let model = c_string(model);
        let deviceid = c_string(deviceid);
        log_msg!(
            LogLevel::Info,
            "receiver: Verbindungsanfrage von {} ({}), ID {}",
            name.as_deref().unwrap_or("?"),
            model.as_deref().unwrap_or("?"),
            deviceid.as_deref().unwrap_or("?")
        );
        let device = name
            .filter(|n| !n.is_empty())
            .or(model.filter(|m| !m.is_empty()))
            .unwrap_or_else(|| "iPhone/iPad".to_string());
        s.events.push(SessionEvent::Connected { device });
    })
}

unsafe extern "C" fn cb_video_process(
    cls: *mut c_void,
    _ntp: *mut raop_ntp_t,
    data: *mut video_decode_struct,
) {
    with_shared(cls, (), |s| {
        let arrival_ns = local_time();
        // SAFETY: UxPlay übergibt eine gültige Struktur oder null.
        let Some(d) = (unsafe { data.as_ref() }) else {
            return;
        };
        s.video.push(VideoPacket {
            data: copy_bytes(d.data, d.data_len),
            h265: d.is_h265,
            ntp_local: s.to_local(d.ntp_time_remote, d.ntp_time_local),
            arrival_ns,
            ntp_remote: d.ntp_time_remote,
        });
    })
}

unsafe extern "C" fn cb_audio_process(
    cls: *mut c_void,
    _ntp: *mut raop_ntp_t,
    data: *mut audio_decode_struct,
) {
    with_shared(cls, (), |s| {
        let arrival_ns = local_time();
        // SAFETY: UxPlay übergibt eine gültige Struktur oder null.
        let Some(d) = (unsafe { data.as_ref() }) else {
            return;
        };
        s.audio.push(AudioPacket {
            data: copy_bytes(d.data, d.data_len),
            ct: if d.ct != 0 {
                d.ct
            } else {
                s.audio_ct.load(Ordering::SeqCst)
            },
            ntp_local: s.to_local(d.ntp_time_remote, d.ntp_time_local),
            arrival_ns,
            rtp_time: d.rtp_time,
            seq: d.seqnum,
        });
    })
}

unsafe extern "C" fn cb_video_set_codec(cls: *mut c_void, codec: video_codec_t) -> c_int {
    with_shared(cls, 0, |s| {
        let h265 = codec == ux::VIDEO_CODEC_H265;
        log_msg!(
            LogLevel::Info,
            "receiver: Videocodec {}",
            if h265 { "H.265" } else { "H.264" }
        );
        s.events.push(SessionEvent::VideoFormat {
            width: 0,
            height: 0,
            h265,
        });
        0
    })
}

unsafe extern "C" fn cb_video_report_size(
    cls: *mut c_void,
    w_src: *mut f32,
    h_src: *mut f32,
    w: *mut f32,
    h: *mut f32,
) {
    with_shared(cls, (), |s| {
        let (w_src, h_src) = (read_or(w_src, 0.0), read_or(h_src, 0.0));
        log_msg!(
            LogLevel::Info,
            "receiver: Quellgröße {:.0}x{:.0}, Anzeige {:.0}x{:.0}",
            w_src,
            h_src,
            read_or(w, 0.0),
            read_or(h, 0.0)
        );
        s.events.push(SessionEvent::VideoFormat {
            width: w_src as i32,
            height: h_src as i32,
            h265: false,
        });
    })
}

unsafe extern "C" fn cb_audio_set_volume(cls: *mut c_void, volume: f32) {
    with_shared(cls, (), |s| {
        s.events.push(SessionEvent::Volume { db: volume });
    })
}

unsafe extern "C" fn cb_audio_set_client_volume(_cls: *mut c_void) -> f64 {
    0.0
}

unsafe extern "C" fn cb_audio_flush(cls: *mut c_void) {
    with_shared(cls, (), |s| s.audio.clear())
}

unsafe extern "C" fn cb_video_flush(cls: *mut c_void) {
    with_shared(cls, (), |s| s.video.clear())
}

unsafe extern "C" fn cb_audio_get_format(
    cls: *mut c_void,
    ct: *mut c_uchar,
    spf: *mut c_ushort,
    using_screen: *mut bool,
    is_media: *mut bool,
    fmt: *mut u64,
) {
    with_shared(cls, (), |s| {
        let ct = read_or(ct, 0);
        s.audio_ct.store(ct, Ordering::SeqCst);
        log_msg!(
            LogLevel::Info,
            "receiver: Audioformat ct={} spf={} screen={} media={} format=0x{:x}",
            ct,
            read_or(spf, 0),
            u8::from(read_or(using_screen, false)),
            u8::from(read_or(is_media, false)),
            read_or(fmt, 0)
        );
    })
}

unsafe extern "C" fn cb_passwd(_cls: *mut c_void, len: *mut c_int) -> *const c_char {
    if !len.is_null() {
        // SAFETY: nicht null, von UxPlay beschreibbar übergeben.
        unsafe { *len = 0 };
    }
    std::ptr::null()
}

unsafe extern "C" fn cb_check_register(_cls: *mut c_void, _pk: *const c_char) -> bool {
    true
}

unsafe extern "C" fn cb_register_client(
    _cls: *mut c_void,
    _device_id: *const c_char,
    _pk: *const c_char,
    _name: *const c_char,
) {
}

unsafe extern "C" fn cb_display_pin(_cls: *mut c_void, _pin: *mut c_char) {}

unsafe extern "C" fn cb_noop(_cls: *mut c_void) {}

unsafe extern "C" fn cb_audio_set_blob(_cls: *mut c_void, _buf: *const c_void, _len: c_int) {}

unsafe extern "C" fn cb_audio_remote_control_id(
    _cls: *mut c_void,
    _dacp_id: *const c_char,
    _active_remote: *const c_char,
) {
}

unsafe extern "C" fn cb_audio_set_progress(
    _cls: *mut c_void,
    _start: *mut u32,
    _curr: *mut u32,
    _end: *mut u32,
) {
}

unsafe extern "C" fn cb_export_dacp(
    _cls: *mut c_void,
    _active_remote: *const c_char,
    _dacp_id: *const c_char,
) {
}

unsafe extern "C" fn cb_mirror_video_running(_cls: *mut c_void, running: bool) {
    let _ = catch_unwind(|| {
        log_msg!(
            LogLevel::Info,
            "receiver: Spiegelung {}",
            if running { "läuft" } else { "beendet" }
        )
    });
}

unsafe extern "C" fn cb_on_video_play(_cls: *mut c_void, _location: *const c_char, _start: f32) {}

unsafe extern "C" fn cb_on_video_float(_cls: *mut c_void, _value: f32) {}

unsafe extern "C" fn cb_on_video_acquire_playback_info(
    _cls: *mut c_void,
    _info: *mut playback_info_t,
) {
}

unsafe extern "C" fn cb_on_video_playlist_remove(_cls: *mut c_void) -> f32 {
    0.0
}

/// Callback-Tabelle wie Receiver::start in v1.
fn callbacks(cls: *mut c_void) -> raop_callbacks_t {
    raop_callbacks_t {
        cls,
        audio_process: Some(cb_audio_process),
        video_process: Some(cb_video_process),
        video_pause: Some(cb_noop),
        video_resume: Some(cb_noop),
        conn_feedback: Some(cb_conn_feedback),
        conn_reset: Some(cb_conn_reset),
        video_reset: Some(cb_video_reset),
        conn_init: Some(cb_conn_init),
        conn_destroy: Some(cb_conn_destroy),
        audio_flush: Some(cb_audio_flush),
        video_flush: Some(cb_video_flush),
        audio_set_client_volume: Some(cb_audio_set_client_volume),
        audio_set_volume: Some(cb_audio_set_volume),
        audio_set_metadata: Some(cb_audio_set_blob),
        audio_set_coverart: Some(cb_audio_set_blob),
        audio_stop_coverart_rendering: Some(cb_noop),
        audio_remote_control_id: Some(cb_audio_remote_control_id),
        audio_set_progress: Some(cb_audio_set_progress),
        audio_get_format: Some(cb_audio_get_format),
        video_report_size: Some(cb_video_report_size),
        mirror_video_running: Some(cb_mirror_video_running),
        report_client_request: Some(cb_report_client_request),
        display_pin: Some(cb_display_pin),
        register_client: Some(cb_register_client),
        check_register: Some(cb_check_register),
        passwd: Some(cb_passwd),
        export_dacp: Some(cb_export_dacp),
        video_set_codec: Some(cb_video_set_codec),
        on_video_play: Some(cb_on_video_play),
        on_video_scrub: Some(cb_on_video_float),
        on_video_rate: Some(cb_on_video_float),
        on_video_stop: Some(cb_noop),
        on_video_acquire_playback_info: Some(cb_on_video_acquire_playback_info),
        on_video_playlist_remove: Some(cb_on_video_playlist_remove),
    }
}
