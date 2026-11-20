#!/usr/bin/env bash
# Baut FFmpeg 9.1 minimal und statisch (nur die Komponenten, die airplay benutzt, LGPL,
# ohne externe Codec-Bibliotheken) nach build/ffmpeg-9.1 bzw. $AIRPLAY_FFMPEG_PREFIX.
# build.rs linkt ausschließlich gegen dieses Präfix. Aus PowerShell:
#   C:\msys64\msys2_shell.cmd -ucrt64 -defterm -no-start -here -c "scripts/build-ffmpeg.sh"
# Werkzeuge einmalig: pacman -S --needed make diffutils mingw-w64-ucrt-x86_64-nasm
# Läuft das Präfix schon mit identischer BUILD-INFO.txt, passiert nichts.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=9.1
URL="https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz"
SHA256=1a4d394ae63d190a526cd052eb4415c156b5c5e2df37efab41a7de0b8fde2433

if [ "${MSYSTEM:-}" != "UCRT64" ]; then
  echo "Abbruch: bitte in der MSYS2-UCRT64-Umgebung ausführen (MSYSTEM=${MSYSTEM:-leer})." >&2
  exit 1
fi
missing=""
for tool in make nasm cmp diff gcc tar xz sha256sum curl cygpath; do
  command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
if [ -n "$missing" ]; then
  echo "Abbruch: Werkzeuge fehlen:$missing" >&2
  echo "Installieren mit: pacman -S --needed make diffutils mingw-w64-ucrt-x86_64-nasm" >&2
  exit 1
fi

# Präfix in Windows-Form mit Schrägstrichen (D:/...): landet so in den .pc-Dateien, die das
# native pkg-config und rustc aus build.rs heraus verstehen müssen.
PREFIX="$(cygpath -am "${AIRPLAY_FFMPEG_PREFIX:-$PWD/build/ffmpeg-$VERSION}")"
SRC_DIR=build/ffmpeg-src
TARBALL="$SRC_DIR/ffmpeg-$VERSION.tar.xz"

CONFIGURE_FLAGS=(
  --prefix="$PREFIX"
  --enable-static --disable-shared
  --disable-programs --disable-doc --disable-network --disable-autodetect
  --disable-everything
  --enable-zlib --enable-d3d11va
  --enable-avcodec --enable-avformat --enable-avutil --enable-swscale --enable-swresample
  --enable-decoder=h264,hevc,aac
  --enable-encoder=aac,png
  --enable-parser=h264,hevc,aac
  --enable-muxer=mp4
  --enable-protocol=file
  --enable-hwaccel=h264_d3d11va,h264_d3d11va2,hevc_d3d11va,hevc_d3d11va2
)

# Landet als licenses/FFmpeg-BUILD-INFO.txt im Programmordner (englisch wie PACKAGES.txt).
BUILD_INFO="FFmpeg $VERSION, built minimal and static for airplay (scripts/build-ffmpeg.sh)
License: LGPL-2.1-or-later (without --enable-gpl, without external codec libraries).
License text: FFmpeg-LGPL-2.1.txt; FFmpeg's notes on files with their own terms: FFmpeg-LICENSE.md
This software is based in part on the work of the Independent JPEG Group.

The Corresponding Source of the bundled FFmpeg is available at
$URL
(unmodified release tarball, SHA256 $SHA256).
It was built with:
./configure ${CONFIGURE_FLAGS[*]}"

INFO_FILE="$PREFIX/share/ffmpeg-build/BUILD-INFO.txt"
if [ -f "$PREFIX/lib/pkgconfig/libavcodec.pc" ] && [ -f "$INFO_FILE" ] \
   && [ "$(cat "$INFO_FILE")" = "$BUILD_INFO" ]; then
  echo "FFmpeg $VERSION ist schon gebaut: $PREFIX (nichts zu tun)"
  exit 0
fi

# Altes Präfix nur löschen, wenn es leer ist oder erkennbar von diesem Skript stammt
# (Markierung share/ffmpeg-build, wird vor make install angelegt).
if [ -d "$PREFIX" ] && [ -n "$(ls -A "$PREFIX")" ] && [ ! -d "$PREFIX/share/ffmpeg-build" ]; then
  echo "Abbruch: $PREFIX ist nicht leer und stammt nicht von build-ffmpeg.sh." >&2
  exit 1
fi

mkdir -p "$SRC_DIR"
if [ ! -f "$TARBALL" ]; then
  echo "Lade $URL"
  curl -fL --retry 3 -o "$TARBALL.part" "$URL"
  mv "$TARBALL.part" "$TARBALL"
fi
actual="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
if [ "$actual" != "$SHA256" ]; then
  echo "Abbruch: SHA256 von $TARBALL stimmt nicht." >&2
  echo "  erwartet: $SHA256" >&2
  echo "  erhalten: $actual" >&2
  echo "Datei prüfen und löschen, dann erneut ausführen." >&2
  exit 1
fi
echo "SHA256 ok: $TARBALL"

# Immer frisch entpacken, damit kein alter Configure-Stand übrig bleibt.
rm -rf "$SRC_DIR/ffmpeg-$VERSION"
tar -xJf "$TARBALL" -C "$SRC_DIR"

rm -rf "$PREFIX"
mkdir -p "$PREFIX/share/ffmpeg-build"
(
  cd "$SRC_DIR/ffmpeg-$VERSION"
  ./configure "${CONFIGURE_FLAGS[@]}"
  make -j"$(nproc)"
  make install
)

cp "$SRC_DIR/ffmpeg-$VERSION/COPYING.LGPLv2.1" "$SRC_DIR/ffmpeg-$VERSION/LICENSE.md" \
  "$PREFIX/share/ffmpeg-build/"
# Zuletzt geschrieben: nur ein vollständiger Build gilt beim nächsten Lauf als fertig.
printf '%s\n' "$BUILD_INFO" > "$INFO_FILE"
echo "Fertig: FFmpeg $VERSION in $PREFIX"
