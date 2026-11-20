//! Windows-Firewall: prüfen (COM `INetFwPolicy2`) und einmalig per UAC eine Regel anlegen.

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, NET_FW_ACTION_ALLOW, NET_FW_PROFILE_TYPE2, NET_FW_PROFILE2_DOMAIN,
    NET_FW_PROFILE2_PRIVATE, NET_FW_PROFILE2_PUBLIC, NET_FW_RULE_DIR_IN, NET_FW_RULE_DIR_OUT,
    NetFwPolicy2,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH, VariantClear};
use windows::Win32::UI::Shell::{
    SEE_MASK_NO_CONSOLE, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::core::{BSTR, Interface, PCWSTR};

use crate::platform::win::paths::to_wide;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirewallResult {
    Added,
    Declined,
    Failed,
}

pub const RULE_NAME: &str = "AirPlay-Empfänger";

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_HANDLE: u32 = 6;
/// winuser.h
const SW_HIDE: i32 = 0;
const STD_PROFILES: i32 =
    NET_FW_PROFILE2_DOMAIN.0 | NET_FW_PROFILE2_PRIVATE.0 | NET_FW_PROFILE2_PUBLIC.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FwRuleInfo {
    pub name: String,
    pub application: String,
    pub allow: bool,
    /// NET_FW_PROFILE2_* Bitmaske
    pub profiles: i32,
    pub inbound: bool,
    pub enabled: bool,
}

/// COM für die Dauer eines Aufrufs; nur wenn CoInitializeEx gelang, wird wieder abgemeldet.
struct ComScope(bool);

impl ComScope {
    fn new() -> Self {
        // SAFETY: keine Vorbedingungen; Gegenstück in Drop.
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: passend zu einem erfolgreichen CoInitializeEx in diesem Thread.
            unsafe { CoUninitialize() };
        }
    }
}

fn policy() -> windows::core::Result<INetFwPolicy2> {
    // SAFETY: COM ist im aufrufenden Thread initialisiert (ComScope).
    unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }
}

/// Eingehende, aktivierte Regel mit diesem Namen vorhanden?
pub fn firewall_rule_exists(rule_name: &str) -> bool {
    let _com = ComScope::new();
    let found = || -> windows::core::Result<bool> {
        // SAFETY: COM-Aufrufe auf gültigen Schnittstellen.
        unsafe {
            let rule = policy()?.Rules()?.Item(&BSTR::from(rule_name))?;
            let dir = rule.Direction().unwrap_or(NET_FW_RULE_DIR_OUT);
            let enabled = rule.Enabled().unwrap_or(VARIANT_FALSE);
            Ok(dir == NET_FW_RULE_DIR_IN && enabled == VARIANT_TRUE)
        }
    };
    found().unwrap_or(false)
}

/// Windows-Pfade: Groß-/Kleinschreibung egal (auch bei Umlauten).
fn same_path(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// Reine Auswertung: Firewall aus → bereit. Sonst braucht es unsere Erlauben-Regel für GENAU
/// dieses Programm auf allen Profilen und keine aktive Block-Regel für das Programm.
pub fn evaluate_firewall(rules: &[FwRuleInfo], exe: &str, rule: &str, firewall_on: bool) -> bool {
    if !firewall_on {
        return true;
    }
    let mut our_rule = false;
    let mut blocked = false;
    for r in rules.iter().filter(|r| r.inbound && r.enabled) {
        let same_app = same_path(&r.application, exe);
        if same_app && !r.allow {
            blocked = true;
        }
        if same_app && r.allow && r.name == rule && (r.profiles & STD_PROFILES) == STD_PROFILES {
            our_rule = true;
        }
    }
    our_rule && !blocked
}

fn rule_info(rule: &INetFwRule) -> Option<FwRuleInfo> {
    // SAFETY: COM-Aufrufe auf einer gültigen Schnittstelle.
    unsafe {
        let app = rule.ApplicationName().ok()?;
        if app.is_empty() {
            return None; // nur programmbezogene Regeln sind relevant
        }
        Some(FwRuleInfo {
            name: rule.Name().map(|n| n.to_string()).unwrap_or_default(),
            application: app.to_string(),
            allow: rule
                .Action()
                .map(|a| a == NET_FW_ACTION_ALLOW)
                .unwrap_or(true),
            profiles: rule.Profiles().unwrap_or(0),
            inbound: rule
                .Direction()
                .map(|d| d == NET_FW_RULE_DIR_IN)
                .unwrap_or(false),
            enabled: rule.Enabled().map(|e| e == VARIANT_TRUE).unwrap_or(false),
        })
    }
}

/// Programmbezogene Regeln und ob die Firewall in einem aktiven Profil an ist.
fn read_rules() -> (Vec<FwRuleInfo>, bool) {
    let mut infos = Vec::new();
    let mut firewall_on = true;
    let Ok(policy) = policy() else {
        return (infos, firewall_on);
    };
    // SAFETY: COM-Aufrufe auf gültigen Schnittstellen; jede VARIANT wird mit VariantClear
    // freigegeben.
    unsafe {
        // Firewall in allen aktuell aktiven Profilen aus → nichts zu tun.
        if let Ok(current) = policy.CurrentProfileTypes()
            && current != 0
        {
            firewall_on = [
                NET_FW_PROFILE2_DOMAIN,
                NET_FW_PROFILE2_PRIVATE,
                NET_FW_PROFILE2_PUBLIC,
            ]
            .into_iter()
            .any(|t: NET_FW_PROFILE_TYPE2| {
                current & t.0 != 0 && policy.get_FirewallEnabled(t) == Ok(VARIANT_TRUE)
            });
        }
        let Ok(it) = policy
            .Rules()
            .and_then(|r| r._NewEnum())
            .and_then(|u| u.cast::<IEnumVARIANT>())
        else {
            return (infos, firewall_on);
        };
        loop {
            let mut v = [VARIANT::default()];
            let mut fetched = 0u32;
            if it.Next(&mut v, &mut fetched).0 != 0 || fetched == 0 {
                break; // S_FALSE: Ende
            }
            let inner = &v[0].Anonymous.Anonymous;
            if inner.vt == VT_DISPATCH
                && let Some(disp) = &*inner.Anonymous.pdispVal
                && let Ok(rule) = disp.cast::<INetFwRule>()
                && let Some(info) = rule_info(&rule)
            {
                infos.push(info);
            }
            let _ = VariantClear(&mut v[0]);
        }
    }
    (infos, firewall_on)
}

/// Unsere Regel existiert für alle Profile UND keine aktive eingehende Block-Regel betrifft
/// das Programm (Windows legt solche beim ersten Start oft selbst an).
pub fn firewall_ready(exe: &str, rule: &str) -> bool {
    let _com = ComScope::new();
    let (infos, firewall_on) = read_rules();
    evaluate_firewall(&infos, exe, rule, firewall_on)
}

/// Argumente für cmd.exe: entfernt alle eingehenden Regeln des Programms und legt eine
/// Erlauben-Regel für alle Profile an, beschränkt auf das lokale Subnetz.
pub fn build_netsh_args(exe: &str, rule: &str) -> String {
    // Löschen schlägt fehl, wenn keine Regel existiert – daher "&" und Exitcode des Anlegens zählt.
    format!(
        "/s /c \"netsh advfirewall firewall delete rule name=all dir=in program=\"{exe}\" >nul 2>&1 & \
         netsh advfirewall firewall add rule name=\"{rule}\" dir=in action=allow program=\"{exe}\" \
         enable=yes profile=any remoteip=localsubnet\""
    )
}

/// Win32-Fehler aus einem windows-Fehler (HRESULT_FROM_WIN32 rückgängig).
fn win32_code(e: &windows::core::Error) -> u32 {
    let hr = e.code().0 as u32;
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        hr & 0xFFFF
    } else {
        hr
    }
}

/// Startet `cmd.exe <args>` mit Admin-Rechten (UAC); liefert (Win32-Fehler des Starts, Exitcode).
fn run_netsh_elevated(args: &str) -> (u32, u32) {
    let verb = to_wide("runas");
    let file = to_wide("cmd.exe");
    let params = to_wide(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE,
        ..Default::default()
    };
    // SAFETY: alle Zeichenketten sind nullterminiert und leben bis nach dem Aufruf.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return (win32_code(&e), 0);
    }
    if info.hProcess.is_invalid() {
        return (ERROR_INVALID_HANDLE, 0);
    }
    let mut exit_code = 0u32;
    // SAFETY: hProcess ist ein gültiges Prozess-Handle (SEE_MASK_NOCLOSEPROCESS), das wir schließen.
    unsafe {
        WaitForSingleObject(info.hProcess, 30000);
        if GetExitCodeProcess(info.hProcess, &mut exit_code).is_err() {
            exit_code = 1;
        }
        let _ = CloseHandle(info.hProcess);
    }
    (ERROR_SUCCESS, exit_code)
}

/// `elevate(args)` startet netsh erhöht und liefert (Win32-Fehler des Starts, Exitcode);
/// ERROR_CANCELLED = UAC abgelehnt.
pub fn add_firewall_rule_elevated_with(
    exe: &str,
    rule: &str,
    elevate: &dyn Fn(&str) -> (u32, u32),
) -> FirewallResult {
    let (rc, exit_code) = elevate(&build_netsh_args(exe, rule));
    if rc == ERROR_CANCELLED.0 {
        FirewallResult::Declined
    } else if rc != ERROR_SUCCESS || exit_code != 0 {
        FirewallResult::Failed
    } else {
        FirewallResult::Added
    }
}

/// Löst eine UAC-Abfrage aus.
pub fn add_firewall_rule_elevated(exe: &str, rule: &str) -> FirewallResult {
    add_firewall_rule_elevated_with(exe, rule, &run_netsh_elevated)
}
