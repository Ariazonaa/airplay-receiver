//! Was nach "Übernehmen" zu tun ist. Ordner brauchen keine Aktion (gelten beim nächsten Speichern).

use crate::app::config::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyEffect {
    ReconfigureScheduler,
    RestartReceiver,
    ToggleStats,
    ChangeLanguage,
}

pub fn diff_config(a: &Config, b: &Config) -> Vec<ApplyEffect> {
    let mut out = Vec::new();
    if a.receiver_name != b.receiver_name || a.adapter != b.adapter || a.allow_h265 != b.allow_h265
    {
        out.push(ApplyEffect::RestartReceiver);
    }
    if a.mode != b.mode || a.smooth_delay_ms != b.smooth_delay_ms {
        out.push(ApplyEffect::ReconfigureScheduler);
    }
    if a.stats_overlay != b.stats_overlay {
        out.push(ApplyEffect::ToggleStats);
    }
    if a.language != b.language {
        out.push(ApplyEffect::ChangeLanguage);
    }
    out
}
