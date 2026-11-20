//! Port von test_session_controller.cpp.
use airplay::app::session_controller::{SessionAction as A, SessionController, SessionState};
use airplay::protocol::packets::SessionEvent;

fn connected(device: &str) -> SessionEvent {
    SessionEvent::Connected {
        device: device.to_string(),
    }
}

fn video_format() -> SessionEvent {
    SessionEvent::VideoFormat {
        width: 0,
        height: 0,
        h265: false,
    }
}

#[test]
fn connect_shows_mirroring() {
    let mut c = SessionController::default();
    assert_eq!(c.state(), SessionState::Idle);
    assert_eq!(
        c.on(&connected("iPhone"), false),
        vec![A::ResetMedia, A::ShowMirroring]
    );
    assert_eq!(c.state(), SessionState::Mirroring);
    assert_eq!(c.device(), "iPhone");
}

#[test]
fn disconnect_returns_to_idle() {
    let mut c = SessionController::default();
    c.on(&connected("iPhone"), false);
    assert_eq!(c.on(&SessionEvent::Disconnected, false), vec![A::ShowIdle]);
    assert_eq!(c.state(), SessionState::Idle);
}

#[test]
fn reset_while_mirroring_returns_to_idle_and_stops_recording() {
    let mut c = SessionController::default();
    c.on(&connected("iPhone"), false);
    assert_eq!(
        c.on(&SessionEvent::Reset, true),
        vec![A::StopRecording, A::ShowIdle]
    );
    assert_eq!(c.state(), SessionState::Idle);
}

#[test]
fn disconnect_while_recording_stops_recording_first() {
    let mut c = SessionController::default();
    c.on(&connected("iPhone"), false);
    assert_eq!(
        c.on(&SessionEvent::Disconnected, true),
        vec![A::StopRecording, A::ShowIdle]
    );
}

#[test]
fn takeover_while_recording() {
    let mut c = SessionController::default();
    c.on(&connected("iPhone"), false);
    assert_eq!(
        c.on(&connected("iPad"), true),
        vec![A::StopRecording, A::ResetMedia, A::ShowMirroring]
    );
    assert_eq!(c.device(), "iPad");
    assert_eq!(c.state(), SessionState::Mirroring);
}

#[test]
fn disconnect_when_idle_is_noop() {
    let mut c = SessionController::default();
    assert!(c.on(&SessionEvent::Disconnected, false).is_empty());
    assert!(c.on(&SessionEvent::Reset, false).is_empty());
}

#[test]
fn video_format_change_keeps_state() {
    let mut c = SessionController::default();
    c.on(&connected("iPhone"), false);
    assert!(c.on(&video_format(), false).is_empty());
    assert_eq!(c.state(), SessionState::Mirroring);
}

#[test]
fn video_format_while_idle_resumes_mirroring_with_last_device() {
    let mut c = SessionController::default();
    c.on(&connected("iPhone"), false);
    c.on(&SessionEvent::Disconnected, false);
    assert_eq!(
        c.on(&video_format(), false),
        vec![A::ResetMedia, A::ShowMirroring]
    );
    assert_eq!(c.state(), SessionState::Mirroring);
    assert_eq!(c.device(), "iPhone");
}

#[test]
fn volume_never_changes_state() {
    let mut c = SessionController::default();
    assert!(c.on(&SessionEvent::Volume { db: -10.0 }, false).is_empty());
    assert_eq!(c.state(), SessionState::Idle);
}

/// Zusatz: Verbindung ohne Gerätenamen behält den bisherigen Namen (wie v1).
#[test]
fn connect_without_name_keeps_previous_device() {
    let mut c = SessionController::default();
    c.on(&connected("iPhone"), false);
    c.on(&connected(""), false);
    assert_eq!(c.device(), "iPhone");
}
