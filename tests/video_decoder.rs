//! Port von test_video_decoder.cpp.

#[path = "support/ci.rs"]
mod ci;
#[path = "support/video_fixture.rs"]
mod video_fixture;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};

use airplay::media::video_decoder::VideoDecoder;
use airplay::protocol::packets::VideoPacket;
use video_fixture::{fresh_temp_dir, make_h264_fixture, split_access_units_default};

#[test]
fn software_decode_of_h264_access_units() {
    let dir = fresh_temp_dir("video");
    assert!(make_h264_fixture(&dir.join("a.h264"), 640, 360, 30));
    let packets = split_access_units_default(&dir.join("a.h264"));
    assert_eq!(packets.len(), 30);

    let mut dec = VideoDecoder::default();
    assert!(dec.open(None, false));
    assert!(!dec.hardware());
    let frames: usize = packets.iter().map(|p| dec.decode(p).len()).sum();
    assert_eq!(frames, 30);
    assert_eq!(dec.width(), 640);
    assert_eq!(dec.height(), 360);
}

#[test]
fn rotation_new_resolution_mid_stream() {
    let dir = fresh_temp_dir("video");
    assert!(make_h264_fixture(&dir.join("a.h264"), 640, 360, 10));
    assert!(make_h264_fixture(&dir.join("b.h264"), 360, 640, 10));
    let a = split_access_units_default(&dir.join("a.h264"));
    let b = split_access_units_default(&dir.join("b.h264"));

    let mut dec = VideoDecoder::default();
    assert!(dec.open(None, false));
    for p in &a {
        dec.decode(p);
    }
    let (mut last_w, mut last_h, mut frames) = (0, 0, 0usize);
    for p in &b {
        for f in dec.decode(p) {
            frames += 1;
            last_w = f.width();
            last_h = f.height();
        }
    }
    assert_eq!(frames, 10);
    assert_eq!(last_w, 360);
    assert_eq!(last_h, 640);
    assert_eq!(dec.width(), 360);
}

fn hardware_device() -> Option<ID3D11Device> {
    let mut dev = None;
    // SAFETY: Ausgabezeiger zeigt auf eine lokale Option.
    let r = unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            None,
        )
    };
    r.ok().and(dev)
}

#[test]
fn hardware_decode_via_d3d11va_on_this_gpu() {
    // Ohne GPU liefert D3D_DRIVER_TYPE_HARDWARE dort ggf. den Basic Render Driver ohne D3D11VA.
    if ci::skip_on_ci(
        "hardware_decode_via_d3d11va_on_this_gpu",
        "GPU mit D3D11VA-Dekodierung",
    ) {
        return;
    }
    let Some(dev) = hardware_device() else {
        println!("kein D3D11-Hardwaregerät – übersprungen");
        return;
    };
    let dir = fresh_temp_dir("video");
    assert!(make_h264_fixture(&dir.join("a.h264"), 1280, 720, 30));
    let packets = split_access_units_default(&dir.join("a.h264"));
    let mut decoder = VideoDecoder::default();
    assert!(decoder.open(Some(&dev), false));
    let mut frames = 0usize;
    let mut kept = Vec::new();
    for p in &packets {
        for f in decoder.decode(p) {
            frames += 1;
            assert!(f.is_hw());
            kept.push(f.clone_ref());
        }
    }
    assert_eq!(frames, 30);
    assert!(decoder.hardware());
    // Festgehaltene GPU-Bilder überleben das Schließen des Decoders (Referenzzählung).
    decoder.close();
    assert!(!decoder.is_open());
    assert!(kept.iter().all(|f| f.is_hw() && f.width() == 1280));
}

#[test]
fn garbage_packet_does_not_crash() {
    let mut dec = VideoDecoder::default();
    assert!(dec.open(None, false));
    let p = VideoPacket {
        data: vec![0, 0, 0, 1, 0x65, 0xff, 0x00, 0x12, 0x34],
        ..Default::default()
    };
    assert!(dec.decode(&p).is_empty());
}
