//! Empfänger zweimal starten und stoppen: UxPlay-Threads dürfen danach keinen freigegebenen
//! Kontext mehr anfassen (Review-Fokus 1).
//!
//! Läuft nur mit einem Adapter mit IPv4 und freiem Port 7000. In der Windows Sandbox gibt es
//! nur einen virtuellen (Hyper-V-)Adapter; für einen Start/Stopp-Test reicht der. Ein
//! Überspringen wird laut gemeldet und ist mit `AIRPLAY_REQUIRE_RECEIVER_TEST=1` ein Fehler
//! (setzt `scripts/sandbox.ps1 test`). Mit `AIRPLAY_CI=1` (GitHub-Runner) wird ebenso laut
//! übersprungen, `AIRPLAY_REQUIRE_RECEIVER_TEST=1` hat auch dann Vorrang.
#[path = "support/ci.rs"]
mod ci;

use std::io::Read;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use airplay::app::log::log_init;
use airplay::discovery::adapters::{NetAdapter, enumerate_adapters, pick_adapter};
use airplay::discovery::dnssd::live_service_instances;
use airplay::protocol::receiver::{Receiver, ReceiverError, ReceiverOptions};

/// Beide Tests brauchen Port 7000 und zählen globale dnssd-Instanzen → nacheinander.
static SERIAL: Mutex<()> = Mutex::new(());

const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

fn port_7000_free() -> bool {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 7000)).is_ok()
}

/// Wie die App ("auto"); sonst irgendein aktiver Adapter mit IPv4, auch ein virtueller.
fn test_adapter() -> Option<NetAdapter> {
    let list = enumerate_adapters();
    pick_adapter(&list, "auto").or_else(|| {
        list.into_iter()
            .find(|a| a.up && !a.ipv4.is_empty() && a.if_type != IF_TYPE_SOFTWARE_LOOPBACK)
    })
}

/// Laut überspringen; mit `AIRPLAY_REQUIRE_RECEIVER_TEST=1` schlägt der Test fehl.
fn skip(reason: &str) {
    ci::report_skip(&format!("Empfänger-Test übersprungen: {reason}"));
    if std::env::var(ci::REQUIRE_VAR).as_deref() == Ok("1") {
        panic!("AIRPLAY_REQUIRE_RECEIVER_TEST=1, Test aber übersprungen: {reason}");
    }
}

/// Adapter und Optionen oder `None` (übersprungen). Das App-Log geht in einen Testordner.
fn setup(test: &str) -> Option<ReceiverOptions> {
    if ci::skip_on_ci(
        &format!("Empfänger-Test {test}"),
        "Port 7000 und einen Netzwerkadapter",
    ) {
        return None;
    }
    let Some(adapter) = test_adapter() else {
        skip("kein aktiver Netzwerkadapter mit IPv4");
        return None;
    };
    if !port_7000_free() {
        skip("Port 7000 ist belegt (läuft der Empfänger schon?)");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("airplay-receiver-test-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    log_init(&dir);
    eprintln!(
        "{test}: Adapter \"{}\" ({}), IPv4 {:?}, Log {}",
        adapter.name,
        adapter.description,
        adapter.ipv4,
        dir.display()
    );
    Some(ReceiverOptions {
        name: "Rust-Test Ä".to_string(),
        adapter,
        allow_h265: false,
        verbose_log: false,
    })
}

/// App-Log des Tests ausgeben (mit `--nocapture` oder bei einem Fehler sichtbar).
fn dump_log(test: &str) {
    let path = std::env::temp_dir()
        .join(format!("airplay-receiver-test-{test}"))
        .join("airplay.log");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    eprintln!("--- {} ---\n{text}--- Ende ---", path.display());
}

fn start_ok(r: &mut Receiver, opts: &ReceiverOptions, what: &str) {
    let t = Instant::now();
    assert!(
        r.start(opts),
        "{what}: Start fehlgeschlagen: {:?}",
        r.last_error()
    );
    eprintln!("{what}: gestartet in {:?}", t.elapsed());
    assert!(r.running());
    assert_eq!(r.last_error(), ReceiverError::None);
    assert_eq!(r.port(), 7000);
    assert_eq!(r.adapter(), &opts.adapter);
    assert!(
        live_service_instances() > 0,
        "{what}: keine Dienst-Instanz angelegt"
    );
}

fn stop_ok(r: &mut Receiver, what: &str) {
    let t = Instant::now();
    r.stop();
    eprintln!("{what}: gestoppt in {:?}", t.elapsed());
    assert!(!r.running());
    assert_eq!(r.open_connections(), 0);
    assert_eq!(
        live_service_instances(),
        0,
        "{what}: Dienst-Instanzen nach dem Abmelden nicht freigegeben"
    );
}

#[test]
fn receiver_restart_twice_does_not_crash() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let test = "restart";
    let Some(opts) = setup(test) else { return };
    let mut r = Receiver::new();
    for round in 0..2 {
        start_ok(&mut r, &opts, &format!("Runde {round}"));
        assert_eq!(r.open_connections(), 0);
        r.poll();
        assert!(r.now_ns() > 0);
        stop_ok(&mut r, &format!("Runde {round}"));
    }
    // Neustart ohne vorheriges stop() und Drop während des Laufs.
    start_ok(&mut r, &opts, "Start 3");
    start_ok(&mut r, &opts, "Start 4 (ohne stop)");
    drop(r);
    assert_eq!(live_service_instances(), 0, "nach Drop nicht freigegeben");
    assert!(port_7000_free(), "Port 7000 nach Drop noch belegt");
    dump_log(test);
}

/// Bis `cond` gilt oder `timeout` abläuft.
fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

/// Verbindet sich mit Port 7000 und wartet, bis UxPlay die Verbindung angenommen hat.
fn connect(r: &Receiver, opts: &ReceiverOptions) -> TcpStream {
    let ip: Ipv4Addr = opts.adapter.ipv4[0].parse().expect("IPv4 des Adapters");
    let c = TcpStream::connect_timeout(&SocketAddrV4::new(ip, 7000).into(), Duration::from_secs(3))
        .expect("Verbindung zu Port 7000");
    assert!(
        wait_for(Duration::from_secs(3), || r.open_connections() >= 1),
        "UxPlay hat die Verbindung nicht angenommen (open_connections {})",
        r.open_connections()
    );
    c
}

/// Nach dem Stopp muss die offene Verbindung vom Empfänger geschlossen sein (EOF oder Reset,
/// kein Lese-Timeout).
fn assert_closed_by_server(mut c: TcpStream, what: &str) {
    c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut buf = [0u8; 64];
    match c.read(&mut buf) {
        Ok(0) => {}
        Ok(n) => panic!("{what}: unerwartete {n} Bytes vom gestoppten Empfänger"),
        Err(e) => assert!(
            !matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "{what}: Verbindung nach dem Stopp noch offen ({e})"
        ),
    }
}

#[test]
fn receiver_restart_with_open_tcp_connection() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let test = "open-connection";
    let Some(opts) = setup(test) else { return };
    let mut r = Receiver::new();
    for round in 0..2 {
        let what = format!("Runde {round} mit Verbindung");
        start_ok(&mut r, &opts, &what);
        let c = connect(&r, &opts);
        eprintln!("{what}: {} offene Verbindung(en)", r.open_connections());
        r.poll();
        stop_ok(&mut r, &what);
        assert_closed_by_server(c, &what);
    }
    // Neustart (ohne stop) und Drop, jeweils mit offener Verbindung.
    start_ok(&mut r, &opts, "Start 3 mit Verbindung");
    let c1 = connect(&r, &opts);
    start_ok(&mut r, &opts, "Start 4 (ohne stop) mit Verbindung");
    assert_closed_by_server(c1, "Neustart ohne stop");
    let c2 = connect(&r, &opts);
    drop(r);
    assert_eq!(live_service_instances(), 0, "nach Drop nicht freigegeben");
    assert_closed_by_server(c2, "Drop");
    dump_log(test);
}
