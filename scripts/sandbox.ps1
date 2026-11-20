<#
Führt Tests oder Programme in einer Windows Sandbox aus statt auf dem Host
(keine Firewall-Abfragen, Ports oder Netzwerk-Ankündigungen auf dem echten PC).

Die Sandbox bleibt zwischen Aufrufen laufen (ID in .superpowers\sandbox-id.txt).
Gemappt: C:\msys64\ucrt64 (nur lesen, DLLs + ffmpeg/ffprobe) und dieses Repo. Der Pfad in der
Sandbox ist derselbe wie auf dem Host (z. B. D:\src\apps\airplay), damit zur Bauzeit eingebrannte
Pfade und die Host-Pfade der Testprogramme stimmen: Auf C: wird direkt gemappt, bei anderen
Laufwerken nach C:\<Laufwerk>\... und dort per subst <Laufwerk>: wieder hergestellt.
Voraussetzung: Windows Sandbox mit wsb-CLI (Windows 11 Pro/Enterprise).

Beispiele (PowerShell, im Repo):
  scripts\sandbox.ps1 test                          # alle Tests bauen + in der Sandbox ausführen
  scripts\sandbox.ps1 test -Cargo '--test video_decoder' -Filter hardware
  scripts\sandbox.ps1 test -Ci                      # wie GitHub-CI: AIRPLAY_CI=1, GPU-/Port-Tests melden Skip
  scripts\sandbox.ps1 run -Exe target\release\airplay.exe -Seconds 20
  scripts\sandbox.ps1 exec -Command 'powershell -File D:\src\apps\airplay\.superpowers\x.ps1'
  scripts\sandbox.ps1 stop
Standard: im angemeldeten Sandbox-Benutzer (Desktop, nötig für Fenster/Swapchain/Bilder-Ordner);
          öffnet beim ersten Mal das Sandbox-Fenster. -System: als SYSTEM ohne Desktop.
#>
param(
    [Parameter(Position = 0, Mandatory = $true)][ValidateSet('test', 'run', 'exec', 'stop')][string]$Action,
    [string]$Cargo = '',   # Argumente für cargo test --no-run, durch Leerzeichen/Komma getrennt
    [string]$Filter = '',
    [string]$Exe = '',
    [string]$ExeArgs = '',
    [int]$Seconds = 20,
    [string]$Command = '',
    [switch]$System,
    [switch]$Ci   # test: AIRPLAY_CI=1 statt AIRPLAY_REQUIRE_RECEIVER_TEST=1 (wie .github/workflows/ci.yml)
)
# 'Continue': wsb schreibt erwartete Fehler (Sitzung noch nicht da) auf stderr; Fehler werden explizit geprüft.
$ErrorActionPreference = 'Continue'
$Repo = (Resolve-Path "$PSScriptRoot\..").Path
$Work = Join-Path $Repo '.superpowers\sandbox'
New-Item -ItemType Directory -Force $Work | Out-Null
$IdFile = Join-Path $Repo '.superpowers\sandbox-id.txt'
$SbRepo = $Repo   # Pfad in der Sandbox nach subst = Host-Pfad
$Drive = $Repo.Substring(0, 1).ToUpper()
if ($Drive -eq 'C') {
    $SbMapped = $Repo                               # direkt auf denselben Pfad mappen
    $SbSubst = ''
} else {
    $SbMapped = "C:\$Drive" + $Repo.Substring(2)    # z. B. C:\D\src\apps\airplay
    $SbSubst = "subst ${Drive}: C:\$Drive >nul 2>&1 & "
}
$SbWork = "$SbRepo\.superpowers\sandbox"

function Get-SandboxId {
    $running = @(wsb list 2>$null | Where-Object { $_ -match '^[0-9a-f-]{36}$' })
    if (Test-Path $IdFile) {
        $id = (Get-Content $IdFile -Raw).Trim()
        if ($running -contains $id) { return $id }
    }
    # Eine fremde Sandbox (andere Sitzung) nicht übernehmen – deren Ordner-Mappings fehlen.
    $hostRepo = $Repo
    $cfg = @"
<Configuration>
  <Networking>Default</Networking>
  <vGPU>Enable</vGPU>
  <MappedFolders>
    <MappedFolder><HostFolder>C:\msys64\ucrt64</HostFolder><SandboxFolder>C:\msys64\ucrt64</SandboxFolder><ReadOnly>true</ReadOnly></MappedFolder>
    <MappedFolder><HostFolder>$hostRepo</HostFolder><SandboxFolder>$SbMapped</SandboxFolder><ReadOnly>false</ReadOnly></MappedFolder>
  </MappedFolders>
</Configuration>
"@
    $out = wsb start -c $cfg --raw 2>&1 | Out-String
    try { $id = ($out | ConvertFrom-Json).Id } catch { $id = $null }
    if (-not $id) {
        Write-Host "Sandbox konnte nicht gestartet werden (läuft eine andere? wsb list). Ausgabe: $out"
        exit 2
    }
    $id | Set-Content $IdFile
    # warten, bis Befehle angenommen werden
    for ($i = 0; $i -lt 60; $i++) {
        wsb exec --id $id -r System -c 'cmd /c exit 0' 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { break }
        Start-Sleep -Seconds 2
    }
    return $id
}

function Invoke-InSandbox([string]$id, [string]$cmdFile, [bool]$interactive) {
    $runAs = 'System'
    if ($interactive) {
        $runAs = 'ExistingLogin'
        wsb exec --id $id -r ExistingLogin -c 'cmd /c exit 0' 2>$null | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Start-Process wsb -ArgumentList @('connect', '--id', $id) | Out-Null
            for ($i = 0; $i -lt 60; $i++) {
                Start-Sleep -Seconds 2
                wsb exec --id $id -r ExistingLogin -c 'cmd /c exit 0' 2>$null | Out-Null
                if ($LASTEXITCODE -eq 0) { break }
            }
        }
    }
    # subst gilt pro Anmeldesitzung → in jedem Aufruf (Fehler "existiert schon" ignorieren)
    $sbCmd = "$SbMapped\.superpowers\sandbox\$(Split-Path $cmdFile -Leaf)"
    wsb exec --id $id -r $runAs -c "cmd /c ${SbSubst}call $sbCmd" | Out-Null
}

function Write-Cmd([string]$name, [string[]]$lines) {
    $path = Join-Path $Work $name
    $header = @('@echo off', 'set PATH=C:\msys64\ucrt64\bin;%PATH%', "set CARGO_MANIFEST_DIR=$SbRepo", "cd /d $SbRepo")
    ($header + $lines) | Set-Content -Encoding Ascii $path
    return $path
}

switch ($Action) {
    'stop' {
        if (Test-Path $IdFile) { wsb stop --id (Get-Content $IdFile -Raw).Trim() 2>$null; Remove-Item $IdFile }
        return
    }
    'test' {
        $cargoArgs = (($Cargo -split '[\s,]+') | Where-Object { $_ } | ForEach-Object { "'$_'" }) -join ' '
        $exes = & C:\msys64\msys2_shell.cmd -ucrt64 -defterm -no-start -here -c "scripts/test-exes.sh $cargoArgs"
        $exes = @($exes | Where-Object { $_ -match '\.exe$' })
        if ($exes.Count -eq 0) { Write-Error 'Keine Testprogramme gebaut (Build-Fehler? scripts/build.sh lokal ohne Ausführen prüfen: cargo test --no-run)'; exit 1 }
        Get-ChildItem $Work -Filter '*.log' -ErrorAction SilentlyContinue | Remove-Item
        # Die Sandbox hat einen Adapter mit IPv4 → tests/receiver.rs darf dort nicht still überspringen.
        # -Ci bildet den GitHub-Runner nach: dort ist nur AIRPLAY_CI=1 gesetzt.
        if ($Ci) { $lines = @('set AIRPLAY_CI=1') } else { $lines = @('set AIRPLAY_REQUIRE_RECEIVER_TEST=1') }
        foreach ($e in $exes) {
            $n = [IO.Path]::GetFileNameWithoutExtension($e)
            $lines += "`"$e`" $Filter > $SbWork\$n.log 2>&1"
            $lines += "echo %ERRORLEVEL% > $SbWork\$n.exit"
        }
        $lines += "echo done > $SbWork\test.done"
        Remove-Item (Join-Path $Work 'test.done') -ErrorAction SilentlyContinue
        $cmd = Write-Cmd 'test.cmd' $lines
        $id = Get-SandboxId
        Invoke-InSandbox $id $cmd (-not $System.IsPresent)
        $failed = 0
        foreach ($e in $exes) {
            $n = [IO.Path]::GetFileNameWithoutExtension($e)
            $code = (Get-Content (Join-Path $Work "$n.exit") -ErrorAction SilentlyContinue | Out-String).Trim()
            $log = Join-Path $Work "$n.log"
            $summary = Select-String -Path $log -Pattern '^test result:' -ErrorAction SilentlyContinue | Select-Object -Last 1
            "{0,-40} exit={1}  {2}" -f $n, $code, $summary.Line
            if ($code -ne '0') {
                $failed++
                Get-Content $log -ErrorAction SilentlyContinue | Select-Object -Last 40
            }
        }
        # Gemeldete Skips (tests/support/ci.rs, tests/receiver.rs) sichtbar machen. Ohne ^: stderr
        # kann mitten in einer "test x ... "-Zeile von libtest (stdout) landen.
        $skips = @(Select-String -Path (Join-Path $Work '*.log') -Pattern 'WARNUNG: .*bersprungen.*' -AllMatches -Encoding UTF8 -ErrorAction SilentlyContinue)
        "Gemeldete Skips: $($skips.Count)"
        foreach ($s in $skips) { "  {0}: {1}" -f [IO.Path]::GetFileNameWithoutExtension($s.Filename), $s.Matches[0].Value }
        "Logs: $Work"
        if ($failed -gt 0) { "FEHLGESCHLAGEN: $failed Testprogramm(e)"; exit 1 }
        "ALLE TESTPROGRAMME OK"
        return
    }
    'run' {
        if (-not $Exe) { Write-Error '-Exe fehlt'; exit 1 }
        $full = (Resolve-Path (Join-Path $Repo $Exe)).Path
        $name = [IO.Path]::GetFileName($full)
        $lines = @(
            "start `"`" `"$full`" $ExeArgs",
            "timeout /t $Seconds /nobreak >nul",
            "taskkill /im $name >nul 2>&1",
            "timeout /t 3 /nobreak >nul",
            "taskkill /f /im $name >nul 2>&1",
            "echo done > $SbWork\run.done"
        )
        $cmd = Write-Cmd 'run.cmd' $lines
        $id = Get-SandboxId
        Invoke-InSandbox $id $cmd (-not $System.IsPresent)
        "fertig: $name lief $Seconds s in der Sandbox. Log der App liegt in der Sandbox unter %LOCALAPPDATA%\airplay\logs (per exec kopieren)."
        return
    }
    'exec' {
        if (-not $Command) { Write-Error '-Command fehlt'; exit 1 }
        $cmd = Write-Cmd 'exec.cmd' @($Command)
        $id = Get-SandboxId
        Invoke-InSandbox $id $cmd (-not $System.IsPresent)
        return
    }
}
