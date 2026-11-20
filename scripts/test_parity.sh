#!/usr/bin/env bash
# Prüft, ob jeder v1-TEST_CASE (C++ auf main) als Rust-#[test] fn existiert.
# Aufruf: scripts/test_parity.sh [v1-testdatei …]   (ohne Argument: alle tests/test_*.cpp)
# Namensregel: Kleinbuchstaben, jede Folge aus Nicht-[a-z0-9] -> "_", Ränder ohne "_".
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$#" -gt 0 ]; then
  files=()
  for f in "$@"; do
    files+=("tests/${f#tests/}")
  done
else
  mapfile -t files < <(git ls-tree --name-only main tests/ | grep -E '^tests/test_[^/]*\.cpp$')
fi

to_snake() {
  printf '%s' "$1" | LC_ALL=C tr '[:upper:]' '[:lower:]' | LC_ALL=C sed -E 's/[^a-z0-9]+/_/g; s/^_+//; s/_+$//'
}

missing=0
total=0
for f in "${files[@]}"; do
  if ! src="$(git show "main:$f" 2>/dev/null)"; then
    echo "FEHLT DATEI: main:$f" >&2
    missing=$((missing + 1))
    continue
  fi
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    fn="$(to_snake "$name")"
    total=$((total + 1))
    if ! grep -rqE "fn ${fn}\(" src tests 2>/dev/null; then
      echo "FEHLT: $f: \"$name\" -> fn $fn"
      missing=$((missing + 1))
    fi
  done < <(printf '%s\n' "$src" | sed -nE 's/^[[:space:]]*TEST_CASE\("([^"]*)".*/\1/p')
done

echo "$total v1-Tests geprüft, $missing fehlen"
[ "$missing" -eq 0 ]
