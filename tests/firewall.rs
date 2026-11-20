//! Port von test_firewall.cpp. Kein Test löst eine echte UAC-Abfrage aus oder ändert die
//! Firewall: Erhöhung läuft immer über einen eingeschleusten Elevator.
use std::cell::RefCell;

use airplay::platform::win::firewall::{
    FirewallResult, FwRuleInfo, RULE_NAME, add_firewall_rule_elevated_with, build_netsh_args,
    evaluate_firewall, firewall_ready, firewall_rule_exists,
};

const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_CANCELLED: u32 = 1223;

#[test]
fn netsh_args_quote_paths_with_spaces() {
    let args = build_netsh_args(
        r"C:\Program Files\Air Play\airplay.exe",
        "AirPlay-Empfänger",
    );
    assert!(args.contains(r#"program="C:\Program Files\Air Play\airplay.exe""#));
    assert!(args.contains(r#"name="AirPlay-Empfänger""#));
    assert!(args.contains("action=allow"));
}

#[test]
fn netsh_args_replace_windows_auto_rules_and_allow_all_profiles_on_local_subnet() {
    let args = build_netsh_args(r"C:\x\airplay.exe", "AirPlay-Empfänger");
    // Läuft über cmd.exe /s /c "…", damit Löschen + Anlegen eine UAC-Abfrage brauchen.
    assert!(args.starts_with("/s /c \""));
    let del = args
        .find(
            r#"netsh advfirewall firewall delete rule name=all dir=in program="C:\x\airplay.exe""#,
        )
        .expect("delete fehlt");
    let add = args
        .find(r#"netsh advfirewall firewall add rule name="AirPlay-Empfänger" dir=in"#)
        .expect("add fehlt");
    assert!(del < add);
    assert!(args.contains("profile=any"));
    assert!(args.contains("remoteip=localsubnet"));
    assert!(!args.contains("profile=private,domain"));
}

#[test]
fn netsh_args_exact_text_like_v1() {
    assert_eq!(
        build_netsh_args(r"C:\x\airplay.exe", RULE_NAME),
        concat!(
            r#"/s /c "netsh advfirewall firewall delete rule name=all dir=in program="C:\x\airplay.exe" >nul 2>&1 & "#,
            r#"netsh advfirewall firewall add rule name="AirPlay-Empfänger" dir=in action=allow program="C:\x\airplay.exe" enable=yes profile=any remoteip=localsubnet""#
        )
    );
}

fn rule(name: &str, app: &str, allow: bool, profiles: i32) -> FwRuleInfo {
    FwRuleInfo {
        name: name.to_string(),
        application: app.to_string(),
        allow,
        profiles,
        inbound: true,
        enabled: true,
    }
}

#[test]
fn our_rule_for_another_program_path_does_not_count() {
    let rules = [rule(
        "AirPlay-Empfänger",
        r"D:\src\apps\airplay\build\airplay.exe",
        true,
        0x7,
    )];
    assert!(!evaluate_firewall(
        &rules,
        r"D:\src\apps\airplay\dist\airplay\airplay.exe",
        "AirPlay-Empfänger",
        true
    ));
}

#[test]
fn our_rule_for_this_program_on_all_profiles_is_ready() {
    let rules = [rule("AirPlay-Empfänger", r"C:\X\AIRPLAY.EXE", true, 0x7)];
    // Groß/klein egal
    assert!(evaluate_firewall(
        &rules,
        r"c:\x\airplay.exe",
        "AirPlay-Empfänger",
        true
    ));
}

#[test]
fn windows_block_rule_for_this_program_wins() {
    let rules = [
        rule("AirPlay-Empfänger", r"C:\x\airplay.exe", true, 0x7),
        rule("airplay", r"C:\x\airplay.exe", false, 0x7),
    ];
    assert!(!evaluate_firewall(
        &rules,
        r"C:\x\airplay.exe",
        "AirPlay-Empfänger",
        true
    ));
}

#[test]
fn rule_limited_to_private_profile_is_not_enough() {
    let rules = [rule("AirPlay-Empfänger", r"C:\x\airplay.exe", true, 0x2)];
    assert!(!evaluate_firewall(
        &rules,
        r"C:\x\airplay.exe",
        "AirPlay-Empfänger",
        true
    ));
}

#[test]
fn disabled_or_outbound_rules_are_ignored() {
    let mut off = rule("airplay", r"C:\x\airplay.exe", false, 0x7);
    off.enabled = false;
    let mut out = rule("airplay", r"C:\x\airplay.exe", false, 0x7);
    out.inbound = false;
    let rules = [
        rule("AirPlay-Empfänger", r"C:\x\airplay.exe", true, 0x7),
        off,
        out,
    ];
    assert!(evaluate_firewall(
        &rules,
        r"C:\x\airplay.exe",
        "AirPlay-Empfänger",
        true
    ));
}

#[test]
fn umlaut_path_compares_case_insensitive() {
    let rules = [rule(
        "AirPlay-Empfänger",
        r"C:\ÄRGER\AIRPLAY.EXE",
        true,
        0x7,
    )];
    assert!(evaluate_firewall(
        &rules,
        r"c:\ärger\airplay.exe",
        "AirPlay-Empfänger",
        true
    ));
}

#[test]
fn firewall_turned_off_needs_no_rule() {
    assert!(evaluate_firewall(
        &[],
        r"C:\x\airplay.exe",
        "AirPlay-Empfänger",
        false
    ));
}

#[test]
fn unknown_program_is_not_firewall_ready() {
    assert!(!firewall_ready(
        r"C:\gibt\es\nicht\airplay-4711.exe",
        "AirPlay-Empfänger-gibt-es-nicht-4711"
    ));
}

#[test]
fn declined_elevation_reports_declined() {
    let declined = |_: &str| (ERROR_CANCELLED, 0);
    assert_eq!(
        add_firewall_rule_elevated_with(r"C:\x\airplay.exe", RULE_NAME, &declined),
        FirewallResult::Declined
    );
}

#[test]
fn netsh_failure_reports_failed() {
    let failing = |_: &str| (ERROR_SUCCESS, 1);
    assert_eq!(
        add_firewall_rule_elevated_with(r"C:\x\airplay.exe", RULE_NAME, &failing),
        FirewallResult::Failed
    );
    let broken = |_: &str| (ERROR_FILE_NOT_FOUND, 0);
    assert_eq!(
        add_firewall_rule_elevated_with(r"C:\x\airplay.exe", RULE_NAME, &broken),
        FirewallResult::Failed
    );
}

#[test]
fn successful_elevation_reports_added_and_passes_args() {
    let seen = RefCell::new(String::new());
    let ok = |args: &str| {
        *seen.borrow_mut() = args.to_string();
        (ERROR_SUCCESS, 0)
    };
    assert_eq!(
        add_firewall_rule_elevated_with(r"C:\x\airplay.exe", RULE_NAME, &ok),
        FirewallResult::Added
    );
    assert_eq!(
        *seen.borrow(),
        build_netsh_args(r"C:\x\airplay.exe", RULE_NAME)
    );
}

#[test]
fn unknown_rule_does_not_exist() {
    assert!(!firewall_rule_exists(
        "AirPlay-Empfänger-gibt-es-nicht-4711"
    ));
}

#[test]
fn rule_name_like_v1() {
    assert_eq!(RULE_NAME, "AirPlay-Empfänger");
}
