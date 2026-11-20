//! Port von tests/video_fixture.h: erzeugt H.264-Testvideos mit der ffmpeg-CLI aus MSYS2 und
//! zerlegt sie in Access Units wie sie UxPlay liefert (Annex-B, Parametersätze vor dem Keyframe).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use airplay::app::demo::split_annexb_access_units;
use airplay::protocol::packets::VideoPacket;

/// Frischer, leerer Ordner pro Aufruf (wie `freshTempDir` aus v1-`test_util.h`).
pub fn fresh_temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join("airplay-tests").join(format!(
        "{}_{}_{}",
        name,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn make_h264_fixture(out: &Path, w: i32, h: i32, frames: i32) -> bool {
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=size={w}x{h}:rate=30"))
        .arg("-frames:v")
        .arg(frames.to_string())
        .args([
            "-c:v",
            "libx264",
            "-bf",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-bsf:v",
            "h264_mp4toannexb",
            "-f",
            "h264",
        ])
        .arg(out)
        .status();
    matches!(status, Ok(s) if s.success()) && out.exists()
}

/// Gruppiert NAL-Einheiten: alles bis einschließlich eines Slice (Typ 1 oder 5) bildet ein Paket
/// (`split_annexb_access_units` aus der Bibliothek); Zeitstempel ab `start_ns` im Abstand `step_ns`.
pub fn split_access_units(file: &Path, start_ns: u64, step_ns: u64) -> Vec<VideoPacket> {
    let d = std::fs::read(file).unwrap_or_default();
    split_annexb_access_units(&d)
        .into_iter()
        .enumerate()
        .map(|(n, data)| VideoPacket {
            data,
            ntp_local: start_ns + n as u64 * step_ns,
            ..Default::default()
        })
        .collect()
}

/// Standardwerte aus v1 (`startNs = 0`, `stepNs = 33333333`).
pub fn split_access_units_default(file: &Path) -> Vec<VideoPacket> {
    split_access_units(file, 0, 33_333_333)
}
