//! Demo-Modus (`airplay.exe --demo <datei.h264>`): spielt eine lokale H.264-Datei (Annex-B)
//! in Endlosschleife durch den echten Bildpfad, als wäre das Gerät "Demo" verbunden. Gedacht
//! für Screenshots ohne iPhone; kein Ton, kein Empfänger, keine Firewall-Prüfung.

use std::path::{Path, PathBuf};

use crate::protocol::packets::VideoPacket;
use crate::record::filenames::contains_keyframe;

/// Befehlszeile. Ohne Argumente: Normalbetrieb.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// H.264-Datei für den Demo-Modus
    pub demo: Option<PathBuf>,
}

/// `--demo <pfad>`; alles andere (unbekannte Argumente, `--demo` ohne Pfad) ist ein Fehler.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Options, String> {
    let mut options = Options::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--demo" => match it.next() {
                Some(path) if !path.is_empty() => options.demo = Some(PathBuf::from(path)),
                _ => return Err("--demo braucht einen Dateipfad (--demo <datei.h264>)".into()),
            },
            _ => return Err(format!("unbekanntes Argument: {arg}")),
        }
    }
    Ok(options)
}

/// Annex-B-H.264 in Bilder zerlegen: alles bis einschließlich eines Slice (NAL-Typ 1 oder 5)
/// bildet eine Einheit – SPS/PPS/SEI hängen also am folgenden Bild (wie UxPlay sie liefert).
/// NAL-Einheiten nach dem letzten Slice werden verworfen.
pub fn split_annexb_access_units(data: &[u8]) -> Vec<Vec<u8>> {
    let d = data;
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 < d.len() {
        let four = d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 0 && d[i + 3] == 1;
        let three = d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 1 && (i == 0 || d[i - 1] != 0);
        if four || three {
            starts.push(i);
        }
        i += 1;
    }
    starts.push(d.len());
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for w in starts.windows(2) {
        let (s, e) = (w[0], w[1]);
        let hdr = if d[s + 2] == 1 { s + 3 } else { s + 4 };
        if hdr >= e {
            continue; // Startcode ohne NAL-Kopf (Dateiende)
        }
        cur.extend_from_slice(&d[s..e]);
        let ty = d[hdr] & 0x1f;
        if ty == 1 || ty == 5 {
            out.push(std::mem::take(&mut cur));
        }
    }
    out
}

/// Bildrate des Demo-Videos
pub const DEMO_FPS: u32 = 60;
/// Größere Lücke (Fenster blockiert, Rechner schlief): Zeitbasis neu setzen statt aufholen.
const MAX_GAP_NS: u64 = 1_000_000_000;

/// Liefert die Bilder einer Datei im festen Takt und von vorn, wenn sie zu Ende ist.
pub struct DemoSource {
    units: Vec<Vec<u8>>,
    fps: u64,
    /// Zeitpunkt von Bild `emitted == 0` der aktuellen Zeitbasis
    base_ns: u64,
    /// seit `base_ns` ausgelieferte Bilder
    emitted: u64,
    /// nächste Einheit
    next: usize,
}

impl DemoSource {
    /// Datei lesen und zerlegen. Fehler bei fehlender/unlesbarer Datei, ohne ein Bild oder wenn
    /// das erste Bild kein Keyframe ist (dann zeigte jede Schleife Artefakte). Die Zeitbasis
    /// wird beim ersten `due` gesetzt.
    pub fn open(path: &Path) -> Result<DemoSource, String> {
        let data = std::fs::read(path)
            .map_err(|e| format!("Demo-Datei {} lässt sich nicht lesen: {e}", path.display()))?;
        let units = split_annexb_access_units(&data);
        let Some(first) = units.first() else {
            return Err(format!(
                "Demo-Datei {} enthält kein H.264-Bild (Annex-B erwartet)",
                path.display()
            ));
        };
        let first = VideoPacket {
            data: first.clone(),
            ..Default::default()
        };
        if !contains_keyframe(&first) {
            return Err(format!(
                "Demo-Datei {}: erstes Bild ist kein Keyframe",
                path.display()
            ));
        }
        // Start 0: der erste `due`-Aufruf ist eine "Lücke" und setzt die Zeitbasis auf jetzt.
        Ok(Self::new(units, DEMO_FPS, 0))
    }

    /// `fps` 0 wird als 1 behandelt.
    pub fn new(units: Vec<Vec<u8>>, fps: u32, start_ns: u64) -> DemoSource {
        DemoSource {
            units,
            fps: u64::from(fps.max(1)),
            base_ns: start_ns,
            emitted: 0,
            next: 0,
        }
    }

    fn scheduled(&self, n: u64) -> u64 {
        self.base_ns + n * 1_000_000_000 / self.fps
    }

    /// Alle bis `now_ns` fälligen Bilder (`ntp_local = arrival_ns =` geplanter Zeitpunkt).
    pub fn due(&mut self, now_ns: u64) -> Vec<VideoPacket> {
        let mut out = Vec::new();
        if self.units.is_empty() {
            return out;
        }
        if now_ns.saturating_sub(self.scheduled(self.emitted)) > MAX_GAP_NS {
            self.base_ns = now_ns;
            self.emitted = 0;
        }
        loop {
            let at = self.scheduled(self.emitted);
            if at > now_ns {
                break;
            }
            out.push(VideoPacket {
                data: self.units[self.next].clone(),
                h265: false,
                ntp_local: at,
                arrival_ns: at,
                ntp_remote: 0,
            });
            self.emitted += 1;
            self.next = (self.next + 1) % self.units.len();
        }
        out
    }
}
