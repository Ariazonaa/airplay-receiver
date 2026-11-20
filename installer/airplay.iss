; Inno-Setup-Skript für AirPlay Receiver.
; Aufruf (normalerweise über scripts\build-installer.ps1):
;   ISCC /DAppVersion=0.1.0 /DOutputName=airplay-receiver-v0.1.0-setup /O<repo>\dist installer\airplay.iss
; Voraussetzung: dist\airplay (aus scripts/bundle.sh).
; Diese Datei muss als UTF-8 mit BOM gespeichert bleiben (Umlaut im Regelnamen).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef OutputName
  #define OutputName "airplay-receiver-setup"
#endif

#define AppExe "airplay.exe"
; Regelname exakt wie in src/platform/win/firewall.rs
#define FwRule "AirPlay-Empfänger"

[Setup]
; Feste AppId (einmal erzeugt, nie ändern – sonst erkennt das Setup eine Vorinstallation nicht):
AppId={{E9433B02-F142-4ACE-9A80-4DEDEB17DAF5}
AppName=AirPlay Receiver
AppVersion={#AppVersion}
AppPublisher=Ariazonaa
AppPublisherURL=https://github.com/Ariazonaa/airplay-receiver
AppSupportURL=https://github.com/Ariazonaa/airplay-receiver/issues
DefaultDirName={autopf}\AirPlay Receiver
DefaultGroupName=AirPlay Receiver
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\assets\icon\airplay.ico
UninstallDisplayIcon={app}\{#AppExe}
CloseApplications=yes
RestartApplications=no
OutputBaseFilename={#OutputName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\dist\airplay\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\AirPlay Receiver"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\AirPlay Receiver"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
; Firewall-Regel wie die App selbst (Regel für den genauen Pfad, nur lokales Subnetz).
; Löschen schlägt ohne vorhandene Regel fehl – das ist gewollt und wird ignoriert.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=all dir=in program=""{app}\{#AppExe}"""; Flags: runhidden waituntilterminated
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""{#FwRule}"" dir=in action=allow program=""{app}\{#AppExe}"" enable=yes profile=any remoteip=localsubnet"; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,AirPlay Receiver}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=all dir=in program=""{app}\{#AppExe}"""; Flags: runhidden waituntilterminated; RunOnceId: "DeleteFirewallRule"
; "airplay-probe.exe --firewall" legt eine gleichnamige Regel für die Probe an – mit entfernen.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=all dir=in program=""{app}\airplay-probe.exe"""; Flags: runhidden waituntilterminated; RunOnceId: "DeleteProbeFirewallRule"

[Code]
// Setup schließt eine laufende App über CloseApplications (Restart Manager), der Uninstaller
// kann das nicht: Ohne das Folgende bliebe airplay.exe laufen und der Programmordner stehen.
// Deshalb hier derselbe Weg von Hand – der Restart Manager bittet die Prozesse, die genau diese
// Dateien benutzen, ums Beenden (die App beendet sich dann sauber), erst danach notfalls hart.
// Setup und Uninstall sind 32-Bit-Prozesse: Zeiger passen in einen Longint.
const
  CCH_RM_SESSION_KEY = 32;
  RmForceShutdown = 1;

function RmStartSession(var SessionHandle: DWORD; SessionFlags: DWORD; SessionKey: String): DWORD;
  external 'RmStartSession@rstrtmgr.dll stdcall';
function RmRegisterResources(SessionHandle: DWORD; nFiles: UINT; var FileNames: Longint;
  nApplications: UINT; Applications: Longint; nServices: UINT; ServiceNames: Longint): DWORD;
  external 'RmRegisterResources@rstrtmgr.dll stdcall';
function RmShutdown(SessionHandle: DWORD; ActionFlags: DWORD; StatusCallback: Longint): DWORD;
  external 'RmShutdown@rstrtmgr.dll stdcall';
function RmEndSession(SessionHandle: DWORD): DWORD;
  external 'RmEndSession@rstrtmgr.dll stdcall';

// Meldet eine Datei beim Restart Manager an (ein Eintrag, Feld mit einem Zeiger).
function RegisterFile(Session: DWORD; FileName: String): DWORD;
var
  P: Longint;
begin
  P := CastStringToInteger(FileName);
  Result := RmRegisterResources(Session, 1, P, 0, 0, 0, 0);
end;

procedure CloseRunningApp;
var
  Session, Res: DWORD;
  Key: String;
begin
  Key := StringOfChar(' ', CCH_RM_SESSION_KEY + 1);
  Res := RmStartSession(Session, 0, Key);
  if Res <> 0 then begin
    Log(Format('Restart Manager: RmStartSession fehlgeschlagen (%d)', [Res]));
    exit;
  end;
  try
    Res := RegisterFile(Session, ExpandConstant('{app}\{#AppExe}'));
    if Res = 0 then
      Res := RegisterFile(Session, ExpandConstant('{app}\airplay-probe.exe'));
    if Res <> 0 then begin
      Log(Format('Restart Manager: RmRegisterResources fehlgeschlagen (%d)', [Res]));
      exit;
    end;
    // Erst höflich beenden lassen; reagiert ein Prozess nicht, hart beenden.
    Res := RmShutdown(Session, 0, 0);
    Log(Format('Restart Manager: Programme beenden, Ergebnis %d', [Res]));
    if Res <> 0 then begin
      Res := RmShutdown(Session, RmForceShutdown, 0);
      Log(Format('Restart Manager: Programme hart beenden, Ergebnis %d', [Res]));
    end;
  finally
    RmEndSession(Session);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    CloseRunningApp;
end;
