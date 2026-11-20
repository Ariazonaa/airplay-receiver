#!/usr/bin/env bash
# Bauen + Clippy (ohne Tests auszuführen). Aus PowerShell:
#   C:\msys64\msys2_shell.cmd -ucrt64 -defterm -no-start -here -c "scripts/build.sh"
# Tests laufen in der Windows Sandbox: powershell -File scripts\sandbox.ps1 test
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --all-targets
cargo-clippy clippy --all-targets -- -D warnings
echo "Gebaut. Tests ausführen: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sandbox.ps1 test"
