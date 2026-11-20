<#
Baut das Setup mit Inno Setup aus dist\airplay (vorher scripts/bundle.sh ausführen).
  scripts\build-installer.ps1 [-Version 0.1.0] [-Tag v0.1.0]
Ergebnis: dist\airplay-receiver-<tag>-setup.exe
#>
param(
    [string]$Version = '',
    [string]$Tag = ''
)
$ErrorActionPreference = 'Stop'
$Repo = (Resolve-Path "$PSScriptRoot\..").Path
if (-not $Version) {
    $m = Select-String -Path (Join-Path $Repo 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $m) { throw 'Version nicht in Cargo.toml gefunden' }
    $Version = $m.Matches[0].Groups[1].Value
}
if (-not $Tag) { $Tag = "v$Version" }

if (-not (Test-Path (Join-Path $Repo 'dist\airplay\airplay.exe'))) {
    throw 'dist\airplay\airplay.exe fehlt – zuerst scripts/bundle.sh ausführen'
}

$iscc = Get-Command ISCC.exe -ErrorAction SilentlyContinue | ForEach-Object { $_.Source }
if (-not $iscc) {
    foreach ($d in @(${env:ProgramFiles(x86)}, $env:ProgramFiles, "$env:LOCALAPPDATA\Programs")) {
        if (-not $d) { continue }
        $c = Join-Path $d 'Inno Setup 6\ISCC.exe'
        if (Test-Path $c) { $iscc = $c; break }
    }
}
if (-not $iscc) { throw 'ISCC.exe nicht gefunden (Inno Setup 6 installieren: winget install JRSoftware.InnoSetup)' }

$name = "airplay-receiver-$Tag-setup"
& $iscc "/DAppVersion=$Version" "/DOutputName=$name" "/O$Repo\dist" (Join-Path $Repo 'installer\airplay.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC fehlgeschlagen (Exit $LASTEXITCODE)" }
$out = Join-Path $Repo "dist\$name.exe"
if (-not (Test-Path $out)) { throw "Setup nicht erzeugt: $out" }
"{0}  ({1:N1} MB)" -f $out, ((Get-Item $out).Length / 1MB)
