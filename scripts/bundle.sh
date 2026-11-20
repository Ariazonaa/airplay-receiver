#!/usr/bin/env bash
# Baut Release und legt ein lauffähiges Programmverzeichnis an (Standard dist/airplay):
# airplay.exe + airplay-probe.exe + benötigte DLLs aus MSYS2 UCRT64 + Lizenzen.
# Aus PowerShell: C:\msys64\msys2_shell.cmd -ucrt64 -defterm -no-start -here -c "scripts/bundle.sh"
# Anderes Ziel: DIST=dist/xyz scripts/bundle.sh
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="${DIST:-dist/airplay}"
OUT="${OUT//\\//}"
# Schutz vor rm -rf an falscher Stelle: erlaubt ist nur ein Unterordner von <repo>/dist
# (nie dist selbst, die Wurzel oder Vorfahren; aufgelöst, Groß/Klein egal).
ROOT_L="$(realpath -m "$PWD" | tr '[:upper:]' '[:lower:]')"
OUT_L="$(realpath -m "$OUT" | tr '[:upper:]' '[:lower:]')"
case "$OUT_L" in
  "$ROOT_L"/dist/?*) ;;
  *) echo "Abbruch: DIST=$OUT muss ein Unterordner von dist/ sein." >&2; exit 1 ;;
esac
[ -n "${BUNDLE_GUARD_ONLY:-}" ] && { echo "Guard ok: $OUT"; exit 0; }
cargo build --release --bins
rm -rf "$OUT"
mkdir -p "$OUT/licenses"
EXES=(target/release/airplay.exe target/release/airplay-probe.exe)
cp "${EXES[@]}" "$OUT/"
# Alle DLLs aus /ucrt64, rekursiv aufgelöst durch ldd.
ldd "${EXES[@]}" | awk '{print $3}' | grep -i '^/ucrt64/' | sort -u | while read -r dll; do
  cp "$dll" "$OUT/"
done
cp LICENSE "$OUT/licenses/airplay-GPL-3.0.txt"
cp third_party/uxplay/LICENSE "$OUT/licenses/UxPlay-GPL-3.0.txt"
# egui (MIT/Apache-2.0) und die von egui eingebetteten Schriften (Ubuntu Font Licence, Hack, egui-icons)
cp third_party/licenses/*.txt "$OUT/licenses/"
cp THIRD-PARTY-LICENSES.txt "$OUT/licenses/"
# Lizenzen der gebündelten MSYS2-DLLs: je DLL das besitzende Paket, dessen Lizenzordner
# nach licenses/dll/<paket>/ und die Zuordnung DLL -> Paket in PACKAGES.txt.
# Standardtexte (third_party/licenses/common) passend zur pacman-Lizenzangabe. Eine
# nackte Angabe "GPL"/"LGPL" ohne Version bekommt alle in Frage kommenden Versionen.
common_texts() {
  local l=" $(echo "$1" | tr '[:upper:]' '[:lower:]') " out=""
  case "$l" in *lgpl-3*|*lgpl3*) out="$out common/LGPL-3.0.txt" ;; esac
  case "$l" in *lgpl-2.1*|*lgpl2.1*) out="$out common/LGPL-2.1.txt" ;; esac
  case "$l" in *" lgpl "*) out="$out common/LGPL-2.1.txt" ;; esac
  case "$l" in *" gpl-2"*|*"spdx:gpl-2"*) out="$out common/GPL-2.0.txt" ;; esac
  case "$l" in *" gpl-3"*|*"spdx:gpl-3"*) out="$out common/GPL-3.0.txt" ;; esac
  case "$l" in *" gpl "*|*" gpl") out="$out common/GPL-2.0.txt common/GPL-3.0.txt" ;; esac
  case "$l" in *apache*) out="$out common/Apache-2.0.txt" ;; esac
  echo "$out" | tr ' ' '\n' | { grep . || true; } | sort -u | tr '\n' ' ' | sed 's/ $//; s/ /, /g'
}
DLL_LIC="$OUT/licenses/dll"
mkdir -p "$DLL_LIC"
mkdir -p "$DLL_LIC/common"
cp third_party/licenses/common/*.txt "$DLL_LIC/common/"
: > "$DLL_LIC/PACKAGES.tmp"
for dll in "$OUT"/*.dll; do
  name="$(basename "$dll")"
  pkg="$(pacman -Qoq "/ucrt64/bin/$name" 2>/dev/null || true)"
  if [ -z "$pkg" ]; then
    echo "Abbruch: kein MSYS2-Paket für $name gefunden." >&2
    exit 1
  fi
  lic="$(pacman -Qi "$pkg" | sed -n 's/^Licenses *: *//p')"
  # Lizenzordner des Pakets laut pacman-Dateiliste (Name weicht teils ab, z. B. SDL3).
  dirs="$(pacman -Qlq "$pkg" | sed -n 's|^/ucrt64/share/licenses/\([^/]*\)/.*|\1|p' | sort -u)"
  for d in $dirs; do
    mkdir -p "$DLL_LIC/$pkg"
    cp -r "/ucrt64/share/licenses/$d/." "$DLL_LIC/$pkg/"
  done
  note=""
  if [ -z "$dirs" ]; then
    # Paket ohne eigene Lizenzdatei: Standardtexte laut pacman-Lizenzangabe zuordnen
    # (liegen einmal unter licenses/dll/common/).
    note="$(common_texts "$lic")"
    # Texte aus dem Upstream-Repo (third_party/licenses/upstream/<name>/, siehe SOURCE.txt).
    short="${pkg#mingw-w64-ucrt-x86_64-}"
    for up in "$short" "${short#lib}"; do
      if [ -d "third_party/licenses/upstream/$up" ]; then
        mkdir -p "$DLL_LIC/$pkg"
        cp -r "third_party/licenses/upstream/$up/." "$DLL_LIC/$pkg/"
        note="${note:+$note, }$pkg/ (Upstream, siehe SOURCE.txt)"
        break
      fi
    done
    if [ -z "$note" ]; then
      note="KEIN LIZENZTEXT VORHANDEN (kein Standardtext fuer diese Lizenz)"
      echo "Warnung: kein Lizenztext fuer $pkg ($name), Lizenz laut pacman: $lic" >&2
    fi
    note=" -> $note"
  fi
  ver="$(pacman -Q "$pkg" | awk '{print $2}')"
  echo "$name $pkg $ver ($lic)$note" >> "$DLL_LIC/PACKAGES.tmp"
done
{
  echo "The DLLs next to airplay.exe come from MSYS2 (UCRT64). Format per line:"
  echo "DLL, package, package version, license (per pacman), license texts."
  echo "The source code of each package version is available at"
  echo "https://repo.msys2.org/mingw/sources/ and https://github.com/msys2/MINGW-packages"
  echo
  sort "$DLL_LIC/PACKAGES.tmp"
} > "$DLL_LIC/PACKAGES.txt"
rm "$DLL_LIC/PACKAGES.tmp"
echo "Fertig: $OUT ($(ls "$OUT" | wc -l) Dateien, $(du -sh "$OUT" | cut -f1))"
