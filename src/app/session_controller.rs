//! Reine Zustandsmaschine: welche Aktionen folgen auf ein Sitzungsereignis.

use crate::protocol::packets::SessionEvent;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionState {
    #[default]
    Idle,
    Mirroring,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAction {
    ShowIdle,
    ShowMirroring,
    ResetMedia,
    StopRecording,
}

#[derive(Clone, Debug, Default)]
pub struct SessionController {
    state: SessionState,
    device: String,
}

impl SessionController {
    pub fn on(&mut self, event: &SessionEvent, recording: bool) -> Vec<SessionAction> {
        let mut actions = Vec::new();
        match event {
            SessionEvent::Connected { device } => {
                if recording {
                    actions.push(SessionAction::StopRecording);
                }
                if !device.is_empty() {
                    self.device = device.clone();
                }
                actions.push(SessionAction::ResetMedia);
                actions.push(SessionAction::ShowMirroring);
                self.state = SessionState::Mirroring;
            }
            SessionEvent::Disconnected | SessionEvent::Reset => {
                if self.state == SessionState::Idle {
                    return actions;
                }
                if recording {
                    actions.push(SessionAction::StopRecording);
                }
                actions.push(SessionAction::ShowIdle);
                self.state = SessionState::Idle;
            }
            SessionEvent::VideoFormat { .. } => {
                // Neuer Videostrom ohne neue Verbindungsanfrage (z. B. Spiegeln erneut gestartet).
                if self.state == SessionState::Idle {
                    actions.push(SessionAction::ResetMedia);
                    actions.push(SessionAction::ShowMirroring);
                    self.state = SessionState::Mirroring;
                }
            }
            SessionEvent::Volume { .. } => {}
        }
        actions
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn device(&self) -> &str {
        &self.device
    }
}
