<#
Testet das Setup (Inno Setup) – nur in der Windows Sandbox ausführen, nie auf dem Host:
  scripts\sandbox.ps1 exec -Command 'powershell -NoProfile -ExecutionPolicy Bypass -File <repo>\scripts\test-installer.ps1 -Setup <repo>\dist\airplay-receiver-v0.1.0-setup.exe'
Ergebnis (PASS/FAIL je Prüfung): .superpowers\installer-test\result.txt, Exit-Code 1 bei FAIL.
Update und Deinstallation laufen jeweils bei laufender App (wie beim Nutzer der Normalfall).
#>
param([Parameter(Mandatory = $true)][string]$Setup)
$ErrorActionPreference = 'Continue'
$Repo = (Resolve-Path "$PSScriptRoot\..").Path
$Out = Join-Path $Repo '.superpowers\installer-test'
New-Item -ItemType Directory -Force $Out | Out-Null
$ResultFile = Join-Path $Out 'result.txt'
Set-Content $ResultFile '' -Encoding UTF8
$script:Fail = 0
function Check([string]$name, [bool]$ok, [string]$detail = '') {
    if ($ok) { $line = "PASS: $name" } else { $script:Fail++; $line = "FAIL: $name" }
    if ($detail) { $line += " ($detail)" }
    Add-Content $ResultFile $line -Encoding UTF8
    Write-Host $line
}

$RuleName = 'AirPlay-Empfänger'
$AppDir = "$env:ProgramFiles\AirPlay Receiver"
$Exe = "$AppDir\airplay.exe"
$Probe = "$AppDir\airplay-probe.exe"
$Unins = "$AppDir\unins000.exe"
$TimeoutSec = 120

function Get-RuleText { (netsh advfirewall firewall show rule name="$RuleName" verbose 2>&1 | Out-String) }
function Get-RuleCount {
    # Zeilen "Rule Name:" / "Regelname:" zählen (Sprache des Systems egal)
    @((Get-RuleText) -split "`r?`n" | Where-Object { $_ -match '^\s*(Rule Name|Regelname)\s*:' }).Count
}
# Startet ein Programm und wartet höchstens $TimeoutSec; Ergebnis: Exit-Code oder 'TIMEOUT'.
function Invoke-Timed([string]$file, [string]$arguments) {
    $p = Start-Process $file -ArgumentList $arguments -PassThru
    $null = $p.Handle   # sonst ist ExitCode nach WaitForExit leer
    if (-not $p.WaitForExit($TimeoutSec * 1000)) {
        try { $p.Kill() } catch {}
        return 'TIMEOUT'
    }
    return $p.ExitCode
}
function Install-Setup([string]$log) {
    Invoke-Timed $Setup "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$log`""
}
function Start-App {
    $p = Start-Process $Exe -PassThru -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 8
    $alive = $false
    $hasWindow = $false
    if ($null -ne $p) {
        $p.Refresh()
        $alive = -not $p.HasExited
        $hasWindow = $alive -and ($p.MainWindowHandle -ne 0)
    }
    return @{ Alive = $alive; Window = $hasWindow }
}
# Wartet bis zu $sec Sekunden, bis kein airplay-Prozess mehr läuft; Ergebnis: Anzahl übrig.
function Wait-NoApp([int]$sec = 10) {
    for ($i = 0; $i -lt $sec -and @(Get-Process airplay -ErrorAction SilentlyContinue).Count -gt 0; $i++) { Start-Sleep -Seconds 1 }
    @(Get-Process airplay -ErrorAction SilentlyContinue).Count
}

if (-not (Test-Path $Setup)) {
    Check 'Setup gefunden' $false "Setup nicht gefunden: $Setup"
    Add-Content $ResultFile 'ERGEBNIS: 1 FAIL' -Encoding UTF8
    exit 1
}
Check 'Setup gefunden' $true $Setup

# Saubere Ausgangslage (sonst Reste einer früheren Installation entfernen)
Get-Process airplay -ErrorAction SilentlyContinue | Stop-Process -Force
if (Test-Path $Unins) { Start-Process $Unins -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -Wait }

try {
# 1. Erstinstallation
$code = Install-Setup (Join-Path $Out 'install1.log')
Check 'Installation (erste) Exit 0' ($code -eq 0) "Exit $code"
Check 'airplay.exe installiert' (Test-Path $Exe) $Exe
Check 'airplay-probe.exe installiert' (Test-Path $Probe)
Check 'DLLs und Lizenzen installiert' ((@(Get-ChildItem $AppDir -Filter *.dll).Count -ge 5) -and (Test-Path "$AppDir\licenses\THIRD-PARTY-LICENSES.txt"))
Check 'Uninstaller vorhanden' (Test-Path $Unins)
$lnk = Get-ChildItem "$env:ProgramData\Microsoft\Windows\Start Menu\Programs" -Recurse -Filter '*.lnk' -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'AirPlay*' }
Check 'Startmenü-Verknüpfung' (@($lnk).Count -ge 1) (($lnk | ForEach-Object { $_.FullName }) -join '; ')

# 2. Firewall-Regel
$rule = Get-RuleText
Set-Content (Join-Path $Out 'rule.txt') $rule -Encoding UTF8
Check 'Firewall-Regel: Programmpfad' ($rule.ToLower().Contains($Exe.ToLower())) $Exe
Check 'Firewall-Regel: LocalSubnet' ($rule -match 'LocalSubnet')
Check 'Firewall-Regel: Richtung In' ($rule -match '(Direction|Richtung)\s*:\s*(In|Eingehend)\b')
Check 'Firewall-Regel: Aktion Allow' ($rule -match '(Action|Aktion)\s*:\s*(Allow|Zulassen)')
Check 'Firewall-Regel: genau eine' ((Get-RuleCount) -eq 1) "Anzahl $(Get-RuleCount)"

# 3. Start der App. Vorher den Zeitstempel der Exe auf einen Merkwert setzen: Inno Setup
# übernimmt beim Ersetzen den Zeitstempel aus dem Setup, so ist "ersetzt" prüfbar.
$oldTime = Get-Date '2001-01-01T00:00:00'
(Get-Item $Exe).LastWriteTime = $oldTime
$app = Start-App
Check 'airplay.exe läuft nach 8 s' $app.Alive
Check 'airplay.exe hat ein Fenster' $app.Window

# 4. Zweite Installation über die bestehende, App läuft dabei (Test a)
$log2 = Join-Path $Out 'install2.log'
$code = Install-Setup $log2
Check 'Update bei laufender App: Exit 0' ($code -eq 0) "Exit $code"
$left = Wait-NoApp
Check 'Update bei laufender App: kein airplay-Prozess mehr' ($left -eq 0) "übrig $left"
$log2Text = if (Test-Path $log2) { Get-Content $log2 -Raw } else { '' }
$rmLines = @($log2Text -split "`r?`n" | Where-Object { $_ -match 'RestartManager|Restart Manager|Shutting down applications' })
Check 'Update bei laufender App: Restart Manager im Log' ($rmLines.Count -ge 1) (($rmLines | ForEach-Object { $_.Trim() }) -join ' | ')
$newTime = if (Test-Path $Exe) { (Get-Item $Exe).LastWriteTime } else { $null }
Check 'Update bei laufender App: airplay.exe ersetzt' (($null -ne $newTime) -and ($newTime -ne $oldTime)) "LastWriteTime $newTime"
Check 'Nach zweiter Installation genau eine Regel' ((Get-RuleCount) -eq 1) "Anzahl $(Get-RuleCount)"

# 5. Nutzerdaten anlegen, damit "bleibt erhalten" prüfbar ist
$Data = "$env:APPDATA\airplay"
New-Item -ItemType Directory -Force $Data | Out-Null
$marker = Join-Path $Data 'installer-test-marker.txt'
Set-Content $marker 'bleibt'

# 6. Regel für airplay-probe.exe anlegen, genau wie "airplay-probe --firewall" (build_netsh_args)
netsh advfirewall firewall add rule name="$RuleName" dir=in action=allow program="$Probe" enable=yes profile=any remoteip=localsubnet | Out-Null
Check 'Probe-Regel angelegt (zwei Regeln)' ((Get-RuleCount) -eq 2) "Anzahl $(Get-RuleCount)"

# 7. Deinstallation, App läuft dabei (Test b)
$app = Start-App
Check 'airplay.exe läuft vor der Deinstallation' $app.Alive
$ulog = Join-Path $Out 'uninstall.log'
$u = Invoke-Timed $Unins "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$ulog`""
Check 'Deinstallation bei laufender App: Exit 0' ($u -eq 0) "Exit $u"
# [Code] in airplay.iss: Restart Manager beendet die App höflich (Ergebnis 0 = ohne Zwang)
$ulogText = if (Test-Path $ulog) { Get-Content $ulog -Raw } else { '' }
$rmLines = @($ulogText -split "`r?`n" | Where-Object { $_ -match 'Restart Manager:' })
Check 'Deinstallation: App über Restart Manager sauber beendet' ([bool]($ulogText -match 'Restart Manager: Programme beenden, Ergebnis 0')) (($rmLines | ForEach-Object { $_.Trim() }) -join ' | ')
# Der Uninstaller räumt über eine Hilfskopie auf; kurz warten
for ($i = 0; $i -lt 30 -and (Test-Path $AppDir); $i++) { Start-Sleep -Seconds 1 }
Check 'Programmordner entfernt' (-not (Test-Path $AppDir)) $(if (Test-Path $AppDir) { (Get-ChildItem $AppDir -Recurse -Name) -join ', ' })
$left = Wait-NoApp
Check 'Deinstallation: kein airplay-Prozess mehr' ($left -eq 0) "übrig $left"
$unProcs = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -like '_iu*' -or $_.Name -like 'unins*' })
Check 'Deinstallation: kein hängender Uninstaller' ($unProcs.Count -eq 0) (($unProcs | ForEach-Object { $_.Name }) -join ', ')
Check 'Firewall-Regeln entfernt (App und Probe)' ((Get-RuleCount) -eq 0) "Anzahl $(Get-RuleCount)"
Check 'Nutzerdaten (%APPDATA%\airplay) bleiben' (Test-Path $marker)
Remove-Item $marker -ErrorAction SilentlyContinue
}
catch {
    Check 'Test ohne Ausnahme' $false "$_"
}
finally {
    # Aufräumen auch bei Ausnahme: App beenden, noch installiertes Programm und Regeln entfernen
    Get-Process airplay -ErrorAction SilentlyContinue | Stop-Process -Force
    if (Test-Path $Unins) {
        Start-Process $Unins -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -Wait
        for ($i = 0; $i -lt 15 -and (Test-Path $AppDir); $i++) { Start-Sleep -Seconds 1 }
    }
    netsh advfirewall firewall delete rule name=all dir=in program="$Probe" | Out-Null
}

$res = if ($script:Fail -eq 0) { 'ALLE PASS' } else { "$($script:Fail) FAIL" }
Add-Content $ResultFile "ERGEBNIS: $res" -Encoding UTF8
if ($script:Fail -gt 0) { exit 1 }
exit 0
