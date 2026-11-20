//! Daten, die der Empfänger aus den UxPlay-Callbacks weiterreicht.

/// Video-Nutzdaten wie von UxPlays `video_process` geliefert (Annex-B).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoPacket {
    pub data: Vec<u8>,
    pub h265: bool,
    /// Zeitbasis von `Receiver::now_ns()`
    pub ntp_local: u64,
    /// Empfangszeit im UxPlay-Callback (gleiche Zeitbasis)
    pub arrival_ns: u64,
    pub ntp_remote: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioPacket {
    pub data: Vec<u8>,
    /// UxPlay-Kompressionstyp
    pub ct: u8,
    pub ntp_local: u64,
    /// Empfangszeit im UxPlay-Callback
    pub arrival_ns: u64,
    pub rtp_time: u32,
    pub seq: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    Connected {
        device: String,
    },
    Disconnected,
    Reset,
    /// Codecwechsel (`width`/`height` 0) oder Quellgröße (`h265` false) – wie v1.
    VideoFormat {
        width: i32,
        height: i32,
        h265: bool,
    },
    Volume {
        db: f32,
    },
}
