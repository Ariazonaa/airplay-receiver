//! Demo-Modus (`--demo <datei.h264>`): Argumente, Zerlegen in Bilder, Takt und Schleife.

#[allow(dead_code)] // hier ohne split_access_units
#[path = "support/video_fixture.rs"]
mod video_fixture;

use std::path::PathBuf;

use airplay::app::demo::{DemoSource, Options, parse_args, split_annexb_access_units};
use airplay::protocol::packets::VideoPacket;
use airplay::record::filenames::contains_keyframe;
use video_fixture::{fresh_temp_dir, make_h264_fixture};

const FRAME_NS: u64 = 16_666_666;

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn fake_units(n: usize) -> Vec<Vec<u8>> {
    (0..n).map(|i| vec![0, 0, 0, 1, 0x65, i as u8]).collect()
}

fn keyframe(data: &[u8]) -> bool {
    contains_keyframe(&VideoPacket {
        data: data.to_vec(),
        ..Default::default()
    })
}

#[test]
fn parse_demo_flag() {
    let o = parse_args(args(&["--demo", "clips\\demo.h264"])).unwrap();
    assert_eq!(o.demo, Some(PathBuf::from("clips\\demo.h264")));
}

#[test]
fn parse_rejects_unknown() {
    assert!(parse_args(args(&["--foo"])).is_err());
    assert!(parse_args(args(&["demo.h264"])).is_err());
    // --demo ohne Pfad
    assert!(parse_args(args(&["--demo"])).is_err());
    assert!(parse_args(args(&["--demo", ""])).is_err());
}

#[test]
fn options_without_demo_keep_receiver() {
    assert_eq!(parse_args(Vec::<String>::new()).unwrap().demo, None);
    assert_eq!(Options::default().demo, None);
}

#[test]
fn split_matches_fixture_frames() {
    let dir = fresh_temp_dir("demo_split");
    let file = dir.join("a.h264");
    assert!(make_h264_fixture(&file, 320, 240, 30));
    let data = std::fs::read(&file).unwrap();
    let units = split_annexb_access_units(&data);
    assert_eq!(units.len(), 30);
    assert!(keyframe(&units[0]), "erstes Bild ist ein Keyframe");
    // Lückenlos: die Einheiten ergeben hintereinander wieder den Dateianfang.
    let joined: Vec<u8> = units.concat();
    assert_eq!(&data[..joined.len()], &joined[..]);
}

#[test]
fn split_handles_empty_and_garbage() {
    assert!(split_annexb_access_units(&[]).is_empty());
    assert!(split_annexb_access_units(&[1, 2, 3, 4, 5]).is_empty());
    // Startcode ganz am Ende ohne NAL-Kopf darf nicht abstürzen.
    assert!(split_annexb_access_units(&[0, 0, 0, 1]).is_empty());
    assert!(split_annexb_access_units(&[0, 0, 0, 1, 0x67, 0, 0, 0, 1]).is_empty());
}

#[test]
fn demo_due_paces_at_60_fps() {
    let mut d = DemoSource::new(fake_units(10), 60, 0);
    let p = d.due(1_000_000_000);
    assert_eq!(p.len(), 61);
    assert_eq!(p[0].ntp_local, 0);
    for w in p.windows(2) {
        let step = w[1].ntp_local - w[0].ntp_local;
        assert!(step.abs_diff(FRAME_NS) <= 1, "Abstand {step}");
    }
    for (i, q) in p.iter().enumerate() {
        assert_eq!(q.ntp_local, q.arrival_ns);
        assert!(!q.h265);
        assert_eq!(q.data, fake_units(10)[i % 10]);
    }
    // schon ausgeliefert → nichts doppelt
    assert!(d.due(1_000_000_000).is_empty());
    // weiter im Takt
    assert_eq!(d.due(1_000_000_000 + FRAME_NS + 1).len(), 1);
}

#[test]
fn demo_loop_restarts_at_keyframe() {
    let dir = fresh_temp_dir("demo_loop");
    let file = dir.join("a.h264");
    // 30 Bilder, x264-Standard keyint 250 > 30: nur das erste Bild ist ein Keyframe.
    assert!(make_h264_fixture(&file, 320, 240, 30));
    let units = split_annexb_access_units(&std::fs::read(&file).unwrap());
    assert_eq!(units.len(), 30);
    assert!(units[1..].iter().all(|u| !keyframe(u)), "keyint = Länge");

    let mut d = DemoSource::open(&file).unwrap();
    let t0 = 1_700_000_000_000_000_000u64; // Zeitbasis wie Receiver::now_ns
    let mut got = d.due(t0);
    assert_eq!(got.len(), 1, "Start ohne Aufholflut");
    got.extend(d.due(t0 + 31 * FRAME_NS + 31));
    assert_eq!(got.len(), 32);
    for (i, p) in got.iter().enumerate() {
        assert_eq!(p.data, units[i % 30], "Bild {i}");
    }
    assert!(
        contains_keyframe(&got[30]),
        "nach dem Ende wieder Einheit 0 mit Keyframe"
    );
    assert!(
        got[30].ntp_local > got[29].ntp_local,
        "Zeit läuft über die Schleife weiter"
    );
}

#[test]
fn demo_missing_file_is_error() {
    let dir = fresh_temp_dir("demo_missing");
    assert!(DemoSource::open(&dir.join("fehlt.h264")).is_err());

    // Datei ohne ein einziges Bild
    let junk = dir.join("junk.h264");
    std::fs::write(&junk, b"kein Video").unwrap();
    assert!(DemoSource::open(&junk).is_err());

    // erstes Bild ohne Keyframe (Anfang abgeschnitten)
    let full = dir.join("a.h264");
    assert!(make_h264_fixture(&full, 320, 240, 5));
    let units = split_annexb_access_units(&std::fs::read(&full).unwrap());
    let cut = dir.join("cut.h264");
    std::fs::write(&cut, units[1..].concat()).unwrap();
    assert!(DemoSource::open(&cut).is_err());

    assert!(DemoSource::open(&full).is_ok());
}

#[test]
fn demo_no_flood_after_gap() {
    let mut d = DemoSource::new(fake_units(10), 60, 0);
    assert_eq!(d.due(0).len(), 1);
    let after_gap = d.due(5_000_000_000);
    assert!(
        after_gap.len() <= 2,
        "{} Pakete nach 5 s Lücke",
        after_gap.len()
    );
    assert!(!after_gap.is_empty());
    // danach wieder normaler Takt ab der neuen Zeitbasis
    let next = d.due(5_000_000_000 + 1_000_000_000);
    assert!(next.len() <= 61 && next.len() >= 59, "{}", next.len());
    // Bildreihenfolge läuft weiter (keine Rückkehr zum Anfang wegen der Lücke)
    assert_eq!(after_gap[0].data, fake_units(10)[1]);
}
