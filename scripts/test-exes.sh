#!/usr/bin/env bash
# Baut die Testprogramme (ohne sie auszuführen) und gibt ihre Pfade aus (Windows-Form).
# Argumente gehen an `cargo test --no-run`, z. B. `--test video_decoder` oder `--release`.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test --no-run --message-format=json "$@" 2>/dev/null \
  | grep '"profile":{[^}]*"test":true' \
  | grep -o '"executable":"[^"]*"' \
  | sed 's/"executable":"//; s/"$//; s#\\\\#\\#g; s#/#\\#g'
