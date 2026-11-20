//! Ersatz für UxPlays `lib/dnssd.c` (v1.73.7). Gleiche C-API (`dnssd.h`) und gleiche
//! TXT-Inhalte, Ankündigung aber über die Windows-DNS-SD-API statt über Apples Bonjour.
//!
//! Die `dnssd_*`-Funktionen werden von UxPlays C-Code beim Linken gefunden. Registrierungs-
//! anfragen und Kontexte liegen im (geboxten) `Dnssd` bzw. sind `'static`, weil die
//! Windows-API sie asynchron weiterbenutzt.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::net::Ipv4Addr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::NetworkManagement::Dns::{
    DNS_SERVICE_INSTANCE, DNS_SERVICE_REGISTER_REQUEST, DnsServiceConstructInstance,
    DnsServiceDeRegister, DnsServiceFreeInstance, DnsServiceRegister,
};
use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
use windows::core::{PCWSTR, PWSTR};

use crate::ffi::uxplay::dnssd_t;

// Konstanten aus dnssd.h / dnssdint.h / global.h
const DNSSD_ERROR_NOERROR: c_int = 0;
const DNSSD_ERROR_HWADDRLEN: c_int = 1;
const RAOP_TXTVERS: &str = "1";
const RAOP_CH: &str = "2";
const RAOP_CN: &str = "0,1,2,3";
const RAOP_ET: &str = "0,3,5";
const RAOP_VV: &str = "2";
const FEATURES_1: u32 = 0x5A7F_FEE6;
const FEATURES_2: u32 = 0x0;
const RAOP_RHD: &str = "5.6.0.0";
const RAOP_SF: &str = "0x4";
const RAOP_SV: &str = "false";
const RAOP_DA: &str = "true";
const RAOP_SR: &str = "44100";
const RAOP_SS: &str = "16";
const RAOP_TP: &str = "UDP";
const RAOP_MD: &str = "0,1,2";
const RAOP_VN: &str = "65537";
const GLOBAL_MODEL: &str = "AppleTV3,2";
const GLOBAL_VERSION: &str = "220.68";
const AIRPLAY_PI: &str = "2e388006-13ba-4041-9a67-25dd4a43d536";

// windns.h
const DNS_QUERY_REQUEST_VERSION1: u32 = 1;
const DNS_REQUEST_PENDING: u32 = 9506;

const MAX_HWADDR: usize = 6;
const MAX_DNS_LABEL: usize = 63;

/// Log-Ziel (level wie UxPlay-Logger: 3=Fehler, 4=Warnung, 6=Info, 7=Debug).
pub type DnssdWinLogFn = extern "C" fn(c_int, *const c_char);

struct Globals {
    if_index: u32,
    ipv4: String,
    log: Option<DnssdWinLogFn>,
}

static GLOBALS: Mutex<Globals> = Mutex::new(Globals {
    if_index: 0,
    ipv4: String::new(),
    log: None,
});

fn globals() -> std::sync::MutexGuard<'static, Globals> {
    GLOBALS.lock().unwrap_or_else(|e| e.into_inner())
}

fn dnssd_log(level: c_int, msg: &str) {
    let f = globals().log;
    if let Some(f) = f {
        let text: Vec<u8> = msg.bytes().filter(|&b| b != 0).chain([0]).collect();
        f(level, text.as_ptr() as *const c_char);
    }
}

/// Führt `f` aus und fängt Panics ab (kein Unwinding in UxPlays C-Code).
fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            dnssd_log(3, "dnssd: interner Fehler (Panik)");
            default
        }
    }
}

/// Verhält sich wie Apples TXTRecordSetValue: ein vorhandener Schlüssel wird entfernt und
/// am Ende neu angehängt.
#[derive(Clone, Default)]
struct TxtRecord {
    entries: Vec<(String, String)>,
}

impl TxtRecord {
    fn set(&mut self, key: &str, value: &str) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| k == key) {
            self.entries.remove(pos);
        }
        self.entries.push((key.to_string(), value.to_string()));
    }

    fn wire(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, v) in &self.entries {
            let mut e = format!("{k}={v}").into_bytes();
            e.truncate(255);
            out.push(e.len() as u8);
            out.extend_from_slice(&e);
        }
        out
    }
}

#[derive(Default)]
struct Registration {
    request: DNS_SERVICE_REGISTER_REQUEST,
    instance: Option<*mut DNS_SERVICE_INSTANCE>,
    active: bool,
}

/// TXT-Eintrag samt fertigem Wire-Format für `dnssd_get_*_txt`.
#[derive(Default)]
struct StoredTxt {
    record: TxtRecord,
    wire: Box<[u8]>,
}

/// Veränderlicher Teil von `Dnssd`, hinter einer Sperre: UxPlays httpd-Thread ruft die
/// Getter, während der Haupt-Thread registriert oder abmeldet.
struct Mutable {
    pk: String,
    features1: u32,
    features2: u32,
    raop_txt: StoredTxt,
    airplay_txt: StoredTxt,
    /// Ersetzte Wire-Puffer. Sie leben bis `dnssd_destroy`, damit Zeiger, die ein Getter
    /// schon herausgegeben hat, gültig bleiben (UxPlay kopiert sie ohne Sperre).
    retired: Vec<Box<[u8]>>,
    raop: Registration,
    airplay: Registration,
}

/// Der von C als `dnssd_t*` gehaltene Zustand. Nach `dnssd_init` werden Name, MAC und
/// pin_pw nicht mehr verändert; alles andere liegt in `m`.
pub struct Dnssd {
    name: Vec<u8>, // nullterminiert
    hwaddr: Vec<u8>,
    pin_pw: u8,
    m: Mutex<Mutable>,
}

fn features_string(m: &Mutable) -> String {
    format!("0x{:X},0x{:X}", m.features1, m.features2)
}

/// Neuen Inhalt übernehmen; der Wire-Puffer wird nur bei geändertem Inhalt ersetzt.
fn store_txt(slot: &mut StoredTxt, retired: &mut Vec<Box<[u8]>>, record: TxtRecord) {
    let wire = record.wire();
    if *slot.wire != wire[..] {
        let old = std::mem::replace(&mut slot.wire, wire.into_boxed_slice());
        if !old.is_empty() {
            retired.push(old);
        }
    }
    slot.record = record;
}

impl Dnssd {
    fn lock(&self) -> std::sync::MutexGuard<'_, Mutable> {
        self.m.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn raop_record(&self, m: &Mutable) -> TxtRecord {
        let mut t = TxtRecord::default();
        t.set("ch", RAOP_CH);
        t.set("cn", RAOP_CN);
        t.set("da", RAOP_DA);
        t.set("et", RAOP_ET);
        t.set("vv", RAOP_VV);
        t.set("ft", &features_string(m));
        t.set("am", GLOBAL_MODEL);
        t.set("md", RAOP_MD);
        t.set("rhd", RAOP_RHD);
        match self.pin_pw {
            2 | 3 => {
                t.set("pw", "true");
                t.set("sf", "0x84");
            }
            1 => {
                t.set("pw", "true");
                t.set("sf", "0x8c");
            }
            _ => {
                t.set("pw", "false");
                t.set("sf", RAOP_SF);
            }
        }
        t.set("sr", RAOP_SR);
        t.set("ss", RAOP_SS);
        t.set("sv", RAOP_SV);
        t.set("tp", RAOP_TP);
        t.set("txtvers", RAOP_TXTVERS);
        t.set("sf", RAOP_SF); // wie im Original: überschreibt den Wert aus dem match
        t.set("vs", GLOBAL_VERSION);
        t.set("vn", RAOP_VN);
        t.set("pk", &m.pk);
        t
    }

    fn airplay_record(&self, m: &Mutable) -> Option<TxtRecord> {
        if self.hwaddr.is_empty() || self.hwaddr.len() > MAX_HWADDR {
            return None;
        }
        let device_id = self
            .hwaddr
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":");
        let mut t = TxtRecord::default();
        t.set("deviceid", &device_id);
        t.set("features", &features_string(m));
        t.set("pw", if self.pin_pw != 0 { "true" } else { "false" });
        t.set("flags", "0x4");
        t.set("model", GLOBAL_MODEL);
        t.set("pk", &m.pk);
        t.set("pi", AIRPLAY_PI);
        t.set("srcvers", GLOBAL_VERSION);
        t.set("vv", "2");
        Some(t)
    }

    /// Beide TXT-Einträge aus pk und Feature-Bits neu bauen (bei `dnssd_init`,
    /// `dnssd_set_pk`, `dnssd_set_airplay_features` und vor dem Registrieren). Die Getter
    /// geben nur noch die gespeicherten Puffer heraus.
    fn rebuild_txt(&self, m: &mut Mutable) {
        let raop = self.raop_record(m);
        store_txt(&mut m.raop_txt, &mut m.retired, raop);
        if let Some(airplay) = self.airplay_record(m) {
            store_txt(&mut m.airplay_txt, &mut m.retired, airplay);
        }
    }
}

fn wide0(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

unsafe extern "system" fn on_register_complete(
    status: u32,
    context: *const c_void,
    instance: *const DNS_SERVICE_INSTANCE,
) {
    let _ = catch_unwind(|| {
        let what = if context.is_null() {
            "?"
        } else {
            // SAFETY: der Kontext ist ein `'static` CStr (siehe `register_service`).
            unsafe { CStr::from_ptr(context as *const c_char) }
                .to_str()
                .unwrap_or("?")
        };
        if status == ERROR_SUCCESS.0 {
            dnssd_log(6, &format!("dnssd: {what} angekündigt"));
        } else {
            dnssd_log(
                3,
                &format!("dnssd: {what} Ankündigung fehlgeschlagen (Status {status})"),
            );
        }
        if !instance.is_null() {
            // SAFETY: eine Kopie, die die API dem Callback übergibt (nicht unsere eigene Instanz
            // aus DnsServiceConstructInstance); der Empfänger muss sie freigeben.
            unsafe { DnsServiceFreeInstance(instance as *mut DNS_SERVICE_INSTANCE) };
        }
    });
}

fn host_name() -> String {
    let mut buf = [0u16; 256];
    let mut len: u32 = 256;
    let mut host = String::from("airplay");
    // SAFETY: buf/len beschreiben einen gültigen Puffer.
    let ok = unsafe {
        GetComputerNameExW(
            ComputerNameDnsHostname,
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
    }
    .is_ok();
    if ok && len > 0 {
        host = String::from_utf16_lossy(&buf[..len as usize]);
    }
    host + ".local"
}

/// Eigene, mit `DnsServiceConstructInstance` erzeugte und noch nicht freigegebene Instanzen.
static LIVE_INSTANCES: AtomicUsize = AtomicUsize::new(0);

/// Anzahl eigener Dienst-Instanzen, die noch nicht freigegeben sind (für Tests: nach dem
/// Abmelden aller Dienste 0).
#[doc(hidden)]
pub fn live_service_instances() -> usize {
    LIVE_INSTANCES.load(Ordering::SeqCst)
}

/// `label` muss `'static` und nullterminiert sein: Die Windows-API ruft den Callback
/// asynchron mit diesem Zeiger als Kontext auf.
fn register_service(
    reg: &mut Registration,
    instance_name: &str,
    ty: &str,
    port: u16,
    txt: &TxtRecord,
    label: &'static CStr,
) -> c_int {
    let (if_index, ipv4) = {
        let g = globals();
        (g.if_index, g.ipv4.clone())
    };
    let keys: Vec<Vec<u16>> = txt.entries.iter().map(|(k, _)| wide0(k)).collect();
    let values: Vec<Vec<u16>> = txt.entries.iter().map(|(_, v)| wide0(v)).collect();
    let key_ptrs: Vec<PCWSTR> = keys.iter().map(|k| PCWSTR(k.as_ptr())).collect();
    let value_ptrs: Vec<PCWSTR> = values.iter().map(|v| PCWSTR(v.as_ptr())).collect();
    let ip4: Option<u32> = ipv4
        .parse::<Ipv4Addr>()
        .ok()
        .map(|a| u32::from_ne_bytes(a.octets()));
    let fqdn = wide0(&service_fqdn(instance_name, ty));
    let host = wide0(&host_name());
    let label_str = label.to_str().unwrap_or("?");

    // SAFETY: alle Zeiger zeigen auf nullterminierte Puffer, die den Aufruf überdauern;
    // DnsServiceConstructInstance kopiert die Daten.
    let inst = unsafe {
        DnsServiceConstructInstance(
            PCWSTR(fqdn.as_ptr()),
            PCWSTR(host.as_ptr()),
            ip4.as_ref().map(|p| p as *const u32),
            None,
            port,
            0,
            0,
            key_ptrs.len() as u32,
            key_ptrs.as_ptr(),
            value_ptrs.as_ptr(),
        )
    };
    if inst.is_null() {
        dnssd_log(
            3,
            &format!("dnssd: DnsServiceConstructInstance fehlgeschlagen für {label_str}"),
        );
        return -3;
    }
    LIVE_INSTANCES.fetch_add(1, Ordering::SeqCst);
    reg.instance = Some(inst);
    reg.request = DNS_SERVICE_REGISTER_REQUEST {
        Version: DNS_QUERY_REQUEST_VERSION1,
        InterfaceIndex: if_index,
        pServiceInstance: inst,
        pRegisterCompletionCallback: Some(on_register_complete),
        pQueryContext: label.as_ptr() as *mut c_void,
        hCredentials: Default::default(),
        unicastEnabled: false.into(),
    };
    // SAFETY: `reg.request` liegt im geboxten `Dnssd` und bleibt bis zur Abmeldung gültig.
    let rc = unsafe { DnsServiceRegister(&reg.request, None) };
    if rc != DNS_REQUEST_PENDING {
        dnssd_log(
            3,
            &format!("dnssd: DnsServiceRegister({label_str}) Fehler {rc}"),
        );
        // SAFETY: die Instanz wurde oben erzeugt und nicht an die API übergeben.
        unsafe { DnsServiceFreeInstance(inst) };
        LIVE_INSTANCES.fetch_sub(1, Ordering::SeqCst);
        reg.instance = None;
        return rc as c_int;
    }
    reg.active = true;
    dnssd_log(
        6,
        &format!(
            "dnssd: registriere {label_str} auf Schnittstelle {if_index} ({}), Port {port}",
            if ipv4.is_empty() { "alle IPs" } else { &ipv4 }
        ),
    );
    0
}

fn unregister_service(reg: &mut Registration) {
    if !reg.active {
        return;
    }
    // SAFETY: dieselbe Anfrage, die bei DnsServiceRegister benutzt wurde.
    unsafe { DnsServiceDeRegister(&reg.request, None) };
    reg.active = false;
    // DnsServiceDeRegister braucht die Anfrage samt pServiceInstance noch (sie benennt den
    // Dienst), danach nicht mehr: Laut Doku erhält der Abschluss-Callback eine *Kopie* der
    // bei DnsServiceRegister übergebenen Instanz (die gibt `on_register_complete` frei), die
    // API hält also eine eigene. Unsere Instanz wird deshalb erst hier freigegeben.
    if let Some(inst) = reg.instance.take() {
        reg.request.pServiceInstance = std::ptr::null_mut();
        // SAFETY: stammt aus DnsServiceConstructInstance und wird nur hier freigegeben.
        unsafe { DnsServiceFreeInstance(inst) };
        LIVE_INSTANCES.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Auf höchstens `max_bytes` kürzen, ohne ein UTF-8-Zeichen zu zerteilen.
pub fn fit_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// "<HWADDR-hex>@<name>" wie im Original, auf das 63-Byte-Limit eines DNS-Labels gekürzt.
pub fn raop_instance_name(hwaddr: &[u8], name: &str) -> String {
    if hwaddr.is_empty() {
        return String::new();
    }
    let hex: String = hwaddr.iter().map(|b| format!("{b:02X}")).collect();
    let prefix = hex.len() + 1; // "<hex>@"
    format!(
        "{hex}@{}",
        fit_utf8(name, MAX_DNS_LABEL.saturating_sub(prefix))
    )
}

pub fn airplay_instance_name(name: &str) -> String {
    fit_utf8(name, MAX_DNS_LABEL).to_string()
}

/// "<instanz mit \. und \ maskiert>.<typ>.local"
pub fn service_fqdn(instance: &str, ty: &str) -> String {
    let mut escaped = String::with_capacity(instance.len() + 8);
    for c in instance.chars() {
        if c == '.' || c == '\\' {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    format!("{escaped}.{ty}.local")
}

/// Vor `dnssd_register_*` aufrufen: Schnittstelle und deren IPv4 (z. B. "192.168.178.42").
///
/// # Safety
/// `ipv4` ist null oder eine nullterminierte C-Zeichenkette.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_win_set_interface(if_index: u32, ipv4: *const c_char) {
    guard((), || {
        let ip = if ipv4.is_null() {
            String::new()
        } else {
            // SAFETY: laut Vertrag nullterminiert.
            unsafe { CStr::from_ptr(ipv4) }
                .to_string_lossy()
                .into_owned()
        };
        let mut g = globals();
        g.if_index = if_index;
        g.ipv4 = ip;
    })
}

/// Optionales Log-Ziel; `None` schaltet das Logging ab.
#[unsafe(no_mangle)]
pub extern "C" fn dnssd_win_set_log(f: Option<DnssdWinLogFn>) {
    globals().log = f;
}

/// # Safety
/// `name` zeigt auf `name_len` Bytes, `hw_addr` auf `hw_addr_len` Bytes, `error` ist null
/// oder beschreibbar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_init(
    name: *const c_char,
    name_len: c_int,
    hw_addr: *const c_char,
    hw_addr_len: c_int,
    error: *mut c_int,
    pin_pw: u8,
) -> *mut dnssd_t {
    guard(std::ptr::null_mut(), || {
        let set_error = |e: c_int| {
            if !error.is_null() {
                // SAFETY: laut Vertrag beschreibbar.
                unsafe { *error = e };
            }
        };
        set_error(DNSSD_ERROR_NOERROR);
        if hw_addr.is_null() || hw_addr_len <= 0 || hw_addr_len as usize > MAX_HWADDR {
            set_error(DNSSD_ERROR_HWADDRLEN);
            return std::ptr::null_mut();
        }
        // SAFETY: Längen laut Vertrag.
        let mut name_bytes = if name.is_null() || name_len <= 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(name as *const u8, name_len as usize) }.to_vec()
        };
        name_bytes.push(0);
        // SAFETY: hw_addr_len wurde oben geprüft.
        let hwaddr =
            unsafe { std::slice::from_raw_parts(hw_addr as *const u8, hw_addr_len as usize) }
                .to_vec();
        let d = Box::new(Dnssd {
            name: name_bytes,
            hwaddr,
            pin_pw,
            m: Mutex::new(Mutable {
                pk: String::new(),
                features1: FEATURES_1,
                features2: FEATURES_2,
                raop_txt: StoredTxt::default(),
                airplay_txt: StoredTxt::default(),
                retired: Vec::new(),
                raop: Registration::default(),
                airplay: Registration::default(),
            }),
        });
        d.rebuild_txt(&mut d.lock());
        Box::into_raw(d) as *mut dnssd_t
    })
}

/// # Safety
/// `d` ist null oder stammt aus `dnssd_init` und wird danach nicht mehr benutzt (auch von
/// keinem anderen Thread mehr; UxPlay ruft das erst nach `raop_destroy`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_destroy(d: *mut dnssd_t) {
    guard((), || {
        if d.is_null() {
            return;
        }
        // SAFETY: laut Vertrag aus Box::into_raw und von niemandem mehr benutzt.
        let d = unsafe { Box::from_raw(d as *mut Dnssd) };
        let mut m = d.lock();
        unregister_service(&mut m.raop);
        unregister_service(&mut m.airplay);
    })
}

/// Gemeinsame (nur lesende) Referenz auf den Zustand; Änderungen laufen über `Dnssd::m`.
/// Es gibt bewusst keine `&mut Dnssd`: UxPlays httpd-Thread ruft die Getter parallel zum
/// Haupt-Thread.
///
/// # Safety
/// `d` ist null oder zeigt auf ein lebendes `Dnssd` aus `dnssd_init`, das während `'a` nicht
/// mit `dnssd_destroy` freigegeben wird.
unsafe fn state<'a>(d: *mut dnssd_t) -> Option<&'a Dnssd> {
    // SAFETY: laut Vertrag null oder ein lebendes `Dnssd`; nur gemeinsame Referenzen.
    unsafe { (d as *const Dnssd).as_ref() }
}

/// # Safety
/// `d` stammt aus `dnssd_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_register_raop(d: *mut dnssd_t, port: u16) -> c_int {
    guard(-1, || {
        // SAFETY: laut Vertrag dieser Funktion.
        let Some(d) = (unsafe { state(d) }) else {
            return -1;
        };
        let instance = raop_instance_name(&d.hwaddr, &String::from_utf8_lossy(name_of(d)));
        if instance.is_empty() {
            return -1;
        }
        let mut guard = d.lock();
        // Wie UxPlay: der angekündigte Eintrag ist der, den die Getter danach liefern.
        d.rebuild_txt(&mut guard);
        let m = &mut *guard;
        unregister_service(&mut m.raop);
        register_service(
            &mut m.raop,
            &instance,
            "_raop._tcp",
            port,
            &m.raop_txt.record,
            c"RAOP (_raop._tcp)",
        )
    })
}

/// # Safety
/// `d` stammt aus `dnssd_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_register_airplay(d: *mut dnssd_t, port: u16) -> c_int {
    guard(-1, || {
        // SAFETY: laut Vertrag dieser Funktion.
        let Some(d) = (unsafe { state(d) }) else {
            return -1;
        };
        let mut guard = d.lock();
        let Some(record) = d.airplay_record(&guard) else {
            return -1;
        };
        let m = &mut *guard;
        store_txt(&mut m.airplay_txt, &mut m.retired, record);
        unregister_service(&mut m.airplay);
        let instance = airplay_instance_name(&String::from_utf8_lossy(name_of(d)));
        register_service(
            &mut m.airplay,
            &instance,
            "_airplay._tcp",
            port,
            &m.airplay_txt.record,
            c"AirPlay (_airplay._tcp)",
        )
    })
}

/// Name ohne abschließende Null.
fn name_of(d: &Dnssd) -> &[u8] {
    &d.name[..d.name.len() - 1]
}

/// # Safety
/// `d` ist null oder stammt aus `dnssd_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_unregister_raop(d: *mut dnssd_t) {
    guard((), || {
        // SAFETY: laut Vertrag dieser Funktion.
        if let Some(d) = unsafe { state(d) } {
            unregister_service(&mut d.lock().raop);
        }
    })
}

/// # Safety
/// `d` ist null oder stammt aus `dnssd_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_unregister_airplay(d: *mut dnssd_t) {
    guard((), || {
        // SAFETY: laut Vertrag dieser Funktion.
        if let Some(d) = unsafe { state(d) } {
            unregister_service(&mut d.lock().airplay);
        }
    })
}

/// # Safety
/// `length` ist null oder zeigt auf einen beschreibbaren `c_int`.
unsafe fn set_len(length: *mut c_int, n: usize) {
    if !length.is_null() {
        // SAFETY: laut Vertrag beschreibbar.
        unsafe { *length = n as c_int };
    }
}

/// Zeiger auf den gespeicherten Wire-Puffer. Er bleibt bis `dnssd_destroy` gültig und
/// unverändert (ersetzte Puffer wandern nach `Mutable::retired`).
///
/// # Safety
/// wie bei `dnssd_get_raop_txt`.
unsafe fn stored_wire(
    d: *mut dnssd_t,
    length: *mut c_int,
    pick: fn(&Mutable) -> &StoredTxt,
) -> *const c_char {
    // SAFETY: laut Vertrag des Aufrufers.
    let Some(d) = (unsafe { state(d) }) else {
        // SAFETY: laut Vertrag des Aufrufers.
        unsafe { set_len(length, 0) };
        return std::ptr::null();
    };
    let m = d.lock();
    let wire = &pick(&m).wire;
    // SAFETY: laut Vertrag des Aufrufers.
    unsafe { set_len(length, wire.len()) };
    if wire.is_empty() {
        std::ptr::null()
    } else {
        wire.as_ptr() as *const c_char
    }
}

/// Der zuletzt gebaute RAOP-TXT-Eintrag (beim Registrieren angekündigt). Baut nichts neu.
///
/// # Safety
/// `d` stammt aus `dnssd_init`, `length` ist null oder beschreibbar. Der Zeiger gilt bis
/// `dnssd_destroy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_get_raop_txt(d: *mut dnssd_t, length: *mut c_int) -> *const c_char {
    // SAFETY: Vertrag wie oben.
    guard(std::ptr::null(), || unsafe {
        stored_wire(d, length, |m| &m.raop_txt)
    })
}

/// Der zuletzt gebaute AirPlay-TXT-Eintrag (beim Registrieren angekündigt). Baut nichts neu.
///
/// # Safety
/// `d` stammt aus `dnssd_init`, `length` ist null oder beschreibbar. Der Zeiger gilt bis
/// `dnssd_destroy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_get_airplay_txt(
    d: *mut dnssd_t,
    length: *mut c_int,
) -> *const c_char {
    // SAFETY: Vertrag wie oben.
    guard(std::ptr::null(), || unsafe {
        stored_wire(d, length, |m| &m.airplay_txt)
    })
}

/// # Safety
/// `d` stammt aus `dnssd_init`, `length` ist null oder beschreibbar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_get_name(d: *mut dnssd_t, length: *mut c_int) -> *const c_char {
    guard(std::ptr::null(), || {
        // SAFETY: laut Vertrag dieser Funktion.
        let Some(d) = (unsafe { state(d) }) else {
            // SAFETY: laut Vertrag dieser Funktion.
            unsafe { set_len(length, 0) };
            return std::ptr::null();
        };
        // SAFETY: laut Vertrag dieser Funktion.
        unsafe { set_len(length, name_of(d).len()) };
        d.name.as_ptr() as *const c_char
    })
}

/// # Safety
/// `d` stammt aus `dnssd_init`, `length` ist null oder beschreibbar.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_get_hw_addr(d: *mut dnssd_t, length: *mut c_int) -> *const c_char {
    guard(std::ptr::null(), || {
        // SAFETY: laut Vertrag dieser Funktion.
        let Some(d) = (unsafe { state(d) }) else {
            // SAFETY: laut Vertrag dieser Funktion.
            unsafe { set_len(length, 0) };
            return std::ptr::null();
        };
        // SAFETY: laut Vertrag dieser Funktion.
        unsafe { set_len(length, d.hwaddr.len()) };
        d.hwaddr.as_ptr() as *const c_char
    })
}

/// # Safety
/// `d` stammt aus `dnssd_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_set_airplay_features(d: *mut dnssd_t, bit: c_int, val: c_int) {
    guard((), || {
        // SAFETY: laut Vertrag dieser Funktion.
        let Some(d) = (unsafe { state(d) }) else {
            return;
        };
        if !(0..=63).contains(&bit) || !(0..=1).contains(&val) {
            return;
        }
        let mut m = d.lock();
        let features = if bit >= 32 {
            &mut m.features2
        } else {
            &mut m.features1
        };
        let mask = 1u32 << (bit % 32);
        if val != 0 {
            *features |= mask;
        } else {
            *features &= !mask;
        }
        d.rebuild_txt(&mut m);
    })
}

/// # Safety
/// `d` stammt aus `dnssd_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_get_airplay_features(d: *mut dnssd_t) -> u64 {
    guard(0, || {
        // SAFETY: laut Vertrag dieser Funktion.
        unsafe { state(d) }.map_or(0, |d| {
            let m = d.lock();
            ((m.features2 as u64) << 32) | m.features1 as u64
        })
    })
}

/// # Safety
/// `d` stammt aus `dnssd_init`, `pk_str` ist null oder nullterminiert.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dnssd_set_pk(d: *mut dnssd_t, pk_str: *mut c_char) {
    guard((), || {
        // SAFETY: laut Vertrag dieser Funktion.
        let Some(d) = (unsafe { state(d) }) else {
            return;
        };
        let pk = if pk_str.is_null() {
            String::new()
        } else {
            // SAFETY: laut Vertrag nullterminiert.
            unsafe { CStr::from_ptr(pk_str) }
                .to_string_lossy()
                .into_owned()
        };
        let mut m = d.lock();
        m.pk = pk;
        d.rebuild_txt(&mut m);
    })
}
