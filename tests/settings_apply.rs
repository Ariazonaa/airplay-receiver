//! Port von test_settings_apply.cpp.
use std::path::PathBuf;

use airplay::app::config::{Config, Mode, default_config};
use airplay::app::settings_apply::{ApplyEffect, diff_config};

fn base() -> Config {
    let mut c = default_config();
    c.receiver_name = "PC".to_string();
    c.screenshot_dir = PathBuf::from("C:\\a");
    c.record_dir = PathBuf::from("C:\\b");
    c
}

#[test]
fn nothing_changed() {
    assert!(diff_config(&base(), &base()).is_empty());
}

#[test]
fn name_change_restarts_receiver() {
    let mut b = base();
    b.receiver_name = "Wohnzimmer".to_string();
    assert_eq!(diff_config(&base(), &b), vec![ApplyEffect::RestartReceiver]);
}

#[test]
fn adapter_or_h265_change_restarts_receiver_once() {
    let mut b = base();
    b.adapter = "Ethernet".to_string();
    b.allow_h265 = true;
    assert_eq!(diff_config(&base(), &b), vec![ApplyEffect::RestartReceiver]);
}

#[test]
fn mode_change_reconfigures_scheduler() {
    let mut b = base();
    b.mode = Mode::Smooth;
    assert_eq!(
        diff_config(&base(), &b),
        vec![ApplyEffect::ReconfigureScheduler]
    );
    let mut c = base();
    c.smooth_delay_ms = 300;
    assert_eq!(
        diff_config(&base(), &c),
        vec![ApplyEffect::ReconfigureScheduler]
    );
}

#[test]
fn name_and_mode_both() {
    let mut b = base();
    b.receiver_name = "X".to_string();
    b.mode = Mode::Smooth;
    let d = diff_config(&base(), &b);
    assert_eq!(d.len(), 2);
    assert!(d.contains(&ApplyEffect::RestartReceiver));
    assert!(d.contains(&ApplyEffect::ReconfigureScheduler));
}

#[test]
fn stats_toggle() {
    let mut b = base();
    b.stats_overlay = true;
    assert_eq!(diff_config(&base(), &b), vec![ApplyEffect::ToggleStats]);
}

#[test]
fn folders_need_no_action() {
    let mut b = base();
    b.screenshot_dir = PathBuf::from("D:\\x");
    b.record_dir = PathBuf::from("D:\\y");
    assert!(diff_config(&base(), &b).is_empty());
}

#[test]
fn language_change_only_changes_language() {
    let mut b = base();
    b.language = "en".to_string();
    assert_eq!(diff_config(&base(), &b), vec![ApplyEffect::ChangeLanguage]);
}
