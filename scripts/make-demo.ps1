<#
Erzeugt das Demo-Video docs\demo\demo.h264 für `airplay.exe --demo` (Screenshots ohne iPhone).

Inhalt: erfundene, neutrale Handy-Oberfläche (dunkler Verlauf, Uhrzeit-Karte, 4x5 Kacheln mit
einfachen Formen, Dock mit 4 Kacheln, wandernder Lichtschein und eine pulsierende Kachel).
590x1280, 60 fps, 6 s (360 Bilder), nahtlose Schleife.

H.264 Annex-B (x264): nur Bild 0 ist ein Keyframe (keyint=360), SPS/PPS davor
(repeat-headers), keine B-Frames und genau ein Slice pro Bild – so zerlegt der Demo-Modus
die Datei in Bilder (src\app\demo.rs).

Pillow läuft in einem temporären uv-venv, das danach wieder gelöscht wird.

Aufruf (PowerShell, im Repo):
  scripts\make-demo.ps1
  scripts\make-demo.ps1 -Ffmpeg C:\msys64\ucrt64\bin\ffmpeg.exe -Out docs\demo\demo.h264
#>
param(
    [string]$Ffmpeg = 'C:\msys64\ucrt64\bin\ffmpeg.exe',
    [string]$Out = ''
)
$ErrorActionPreference = 'Stop'
$Repo = (Resolve-Path "$PSScriptRoot\..").Path
if (-not $Out) { $Out = Join-Path $Repo 'docs\demo\demo.h264' }
$Out = [IO.Path]::GetFullPath($Out)
New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
if (-not (Get-Command $Ffmpeg -ErrorAction SilentlyContinue)) { throw "ffmpeg not found: $Ffmpeg" }
if (-not (Get-Command uv -ErrorAction SilentlyContinue)) { throw 'uv not found (https://docs.astral.sh/uv/)' }

# Nur ASCII im Python-Teil: Windows PowerShell 5.1 liest Skripte ohne BOM als ANSI.
$py = @'
import math
import subprocess
import sys

from PIL import Image, ImageDraw, ImageFilter, ImageFont

W, H, FPS, N = 590, 1280, 60, 360
S = 2  # supersampling for smooth edges
out_path, ffmpeg = sys.argv[1], sys.argv[2]
FONTS = "C:\\Windows\\Fonts\\"


def font(name, size):
    try:
        return ImageFont.truetype(FONTS + name, size)
    except OSError:
        return ImageFont.load_default(size)


def lerp(a, b, t):
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3))


def shrink(img, size):
    # premultiplied alpha, so transparent edges stay clean
    return img.convert("RGBa").resize(size, Image.LANCZOS).convert("RGBA")


# --- background: dark vertical gradient --------------------------------------------------
bg = Image.new("RGB", (W, H))
d = ImageDraw.Draw(bg)
top, bottom = (24, 27, 48), (10, 30, 38)
for y in range(H):
    d.line([(0, y), (W, y)], fill=lerp(top, bottom, y / (H - 1)))


def glow_sprite(size, color, strength):
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).ellipse([size * 0.25, size * 0.25, size * 0.75, size * 0.75], fill=255)
    m = m.filter(ImageFilter.GaussianBlur(size * 0.11))
    g = Image.new("RGBA", (size, size), color + (0,))
    g.putalpha(m.point(lambda v: int(v * strength)))
    return g


GLOW_A = glow_sprite(780, (60, 170, 185), 0.42)
GLOW_B = glow_sprite(640, (130, 90, 200), 0.32)

# --- layout -----------------------------------------------------------------------------
TILE = 102
GAP = 34
LEFT = (W - 4 * TILE - 3 * GAP) // 2
GRID_TOP = 300
DOCK_TOP = 1090
DOCK_H = 150
RADIUS = 30

COLORS = [
    (231, 111, 81), (244, 162, 97), (233, 196, 106), (42, 157, 143),
    (38, 70, 83), (94, 129, 172), (129, 178, 154), (224, 122, 95),
    (61, 90, 128), (152, 193, 217), (238, 108, 77), (106, 76, 147),
    (25, 130, 196), (138, 201, 38), (255, 89, 94), (255, 202, 58),
    (82, 166, 117), (199, 125, 255), (72, 202, 228), (176, 137, 104),
]
DOCK_COLORS = [(46, 196, 182), (255, 159, 28), (114, 9, 183), (247, 37, 133)]
SHAPES = [
    "circle", "triangle", "square", "diamond", "ring", "plus", "hexagon", "star",
    "bars", "dots", "moon", "cross", "chevron", "frame", "pentagon", "slider",
    "quad", "tri_outline", "arrow", "wave",
]
DOCK_SHAPES = ["circle", "square", "triangle", "dots"]
PULSE = (2, 1)  # (column, row) of the pulsing tile


def poly(cx, cy, r, n, rot=-math.pi / 2, inner=None):
    pts = []
    steps = n * 2 if inner else n
    for k in range(steps):
        a = rot + 2 * math.pi * k / steps
        rr = r if not inner or k % 2 == 0 else inner
        pts.append((cx + rr * math.cos(a), cy + rr * math.sin(a)))
    return pts


def draw_shape(d, kind, cx, cy, r, bgc):
    fg = (255, 255, 255, 235)
    w = max(2, int(r * 0.28))
    if kind == "circle":
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=fg)
    elif kind == "triangle":
        d.polygon(poly(cx, cy + r * 0.15, r * 1.1, 3), fill=fg)
    elif kind == "square":
        q = r * 0.85
        d.rounded_rectangle([cx - q, cy - q, cx + q, cy + q], radius=r * 0.18, fill=fg)
    elif kind == "diamond":
        d.polygon(poly(cx, cy, r * 1.1, 4), fill=fg)
    elif kind == "ring":
        d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=fg, width=w)
    elif kind == "plus":
        d.rounded_rectangle([cx - w / 2 - 2, cy - r, cx + w / 2 + 2, cy + r], radius=w / 2, fill=fg)
        d.rounded_rectangle([cx - r, cy - w / 2 - 2, cx + r, cy + w / 2 + 2], radius=w / 2, fill=fg)
    elif kind == "hexagon":
        d.polygon(poly(cx, cy, r, 6, rot=0), fill=fg)
    elif kind == "star":
        d.polygon(poly(cx, cy + r * 0.05, r * 1.1, 5, inner=r * 0.48), fill=fg)
    elif kind == "bars":
        bw = r * 0.38
        for i, h in enumerate((0.55, 1.0, 0.75)):
            x0 = cx - r + i * (bw + r * 0.43)
            d.rounded_rectangle([x0, cy + r - 2 * r * h, x0 + bw, cy + r], radius=bw / 3, fill=fg)
    elif kind == "dots":
        dr = r * 0.26
        for i in (-1, 0, 1):
            x = cx + i * r * 0.72
            d.ellipse([x - dr, cy - dr, x + dr, cy + dr], fill=fg)
    elif kind == "moon":
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=fg)
        o = r * 0.55
        d.ellipse([cx - r + o, cy - r - o * 0.4, cx + r + o, cy + r - o * 0.4], fill=bgc + (255,))
    elif kind == "cross":
        d.line([(cx - r * 0.8, cy - r * 0.8), (cx + r * 0.8, cy + r * 0.8)], fill=fg, width=w)
        d.line([(cx - r * 0.8, cy + r * 0.8), (cx + r * 0.8, cy - r * 0.8)], fill=fg, width=w)
    elif kind == "chevron":
        d.line([(cx - r * 0.5, cy - r * 0.9), (cx + r * 0.4, cy), (cx - r * 0.5, cy + r * 0.9)],
               fill=fg, width=w, joint="curve")
    elif kind == "frame":
        q = r * 0.9
        d.rounded_rectangle([cx - q, cy - q, cx + q, cy + q], radius=r * 0.3, outline=fg, width=w)
    elif kind == "pentagon":
        d.polygon(poly(cx, cy + r * 0.05, r * 1.05, 5), fill=fg)
    elif kind == "slider":
        d.rounded_rectangle([cx - r, cy - w / 4, cx + r, cy + w / 4], radius=w / 4, fill=fg)
        kr = r * 0.36
        d.ellipse([cx + r * 0.2 - kr, cy - kr, cx + r * 0.2 + kr, cy + kr], fill=fg)
    elif kind == "quad":
        q = r * 0.42
        for dx in (-1, 1):
            for dy in (-1, 1):
                x, y = cx + dx * r * 0.5, cy + dy * r * 0.5
                d.rounded_rectangle([x - q, y - q, x + q, y + q], radius=q * 0.3, fill=fg)
    elif kind == "tri_outline":
        d.polygon(poly(cx, cy + r * 0.15, r * 1.1, 3), outline=fg, width=w)
    elif kind == "arrow":
        d.polygon([(cx, cy - r), (cx + r * 0.85, cy), (cx - r * 0.85, cy)], fill=fg)
        d.rounded_rectangle([cx - w / 2, cy - 2, cx + w / 2, cy + r], radius=w / 3, fill=fg)
    elif kind == "wave":
        pts = [(cx - r + 2 * r * k / 40, cy + r * 0.45 * math.sin(2 * math.pi * k / 40 * 1.5))
               for k in range(41)]
        d.line(pts, fill=fg, width=w, joint="curve")


def tile_image(color, shape, size):
    """One tile at supersampled size `size` (pixels), transparent margin around it."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, size - 1, size - 1], radius=RADIUS * size / TILE, fill=color + (255,))
    draw_shape(d, shape, size / 2, size / 2, size * 0.24, color)
    return img


def tile_pos(col, row):
    return LEFT + col * (TILE + GAP), GRID_TOP + row * (TILE + GAP)


# --- static layer (supersampled, then shrunk once) --------------------------------------
layer = Image.new("RGBA", (W * S, H * S), (0, 0, 0, 0))
d = ImageDraw.Draw(layer)

# clock card
cx0, cy0, cx1, cy1 = LEFT, 80, W - LEFT, 230
d.rounded_rectangle([cx0 * S, cy0 * S, cx1 * S, cy1 * S], radius=34 * S, fill=(255, 255, 255, 22))
f_clock = font("segoeuisl.ttf", 92 * S)
f_small = font("segoeui.ttf", 26 * S)
d.text(((cx0 + 30) * S, (cy0 + 75) * S), "12:30", font=f_clock, fill=(240, 244, 248, 255), anchor="lm")
d.text(((cx1 - 30) * S, (cy0 + 75) * S), "Demo", font=f_small, fill=(170, 186, 200, 255), anchor="rm")

# grid tiles
for row in range(5):
    for col in range(4):
        if (col, row) == PULSE:
            continue
        i = row * 4 + col
        x, y = tile_pos(col, row)
        layer.alpha_composite(tile_image(COLORS[i], SHAPES[i], TILE * S), (x * S, y * S))

# page indicator: three small dots
for k in range(3):
    x = W / 2 + (k - 1) * 22
    y = GRID_TOP + 5 * (TILE + GAP) + 20
    a = 230 if k == 0 else 90
    d.ellipse([(x - 5) * S, (y - 5) * S, (x + 5) * S, (y + 5) * S], fill=(255, 255, 255, a))

# dock
d.rounded_rectangle([(LEFT - 20) * S, DOCK_TOP * S, (W - LEFT + 20) * S, (DOCK_TOP + DOCK_H) * S],
                    radius=44 * S, fill=(255, 255, 255, 30))
for col in range(4):
    x = LEFT + col * (TILE + GAP)
    y = DOCK_TOP + (DOCK_H - TILE) // 2
    layer.alpha_composite(tile_image(DOCK_COLORS[col], DOCK_SHAPES[col], TILE * S), (x * S, y * S))

static = shrink(layer, (W, H))

# pulsing tile and its halo, kept supersampled and scaled per frame
p_idx = PULSE[1] * 4 + PULSE[0]
pulse_src = tile_image(COLORS[p_idx], SHAPES[p_idx], TILE * S * 2)
HALO = TILE + 80
halo = Image.new("L", (HALO, HALO), 0)
ImageDraw.Draw(halo).rounded_rectangle([40, 40, HALO - 41, HALO - 41], radius=RADIUS + 6, fill=255)
halo = halo.filter(ImageFilter.GaussianBlur(14))
halo_rgb = Image.new("RGBA", (HALO, HALO), COLORS[p_idx] + (0,))

# --- encode -----------------------------------------------------------------------------
cmd = [
    ffmpeg, "-hide_banner", "-loglevel", "error", "-y",
    "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-",
    "-vf", "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
    "-c:v", "libx264", "-preset", "slow", "-crf", "18", "-bf", "0", "-g", str(N),
    "-x264-params", f"keyint={N}:min-keyint={N}:scenecut=0:repeat-headers=1:sliced-threads=0:slices=1"
    ":colorprim=bt709:transfer=bt709:colormatrix=bt709",
    "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv",
    "-f", "h264", out_path,
]
enc = subprocess.Popen(cmd, stdin=subprocess.PIPE)
px, py = tile_pos(*PULSE)
for i in range(N):
    t = i / N  # one full loop = 2*pi, so frame N equals frame 0
    a = 2 * math.pi * t
    frame = bg.copy()
    gx = W * 0.5 + W * 0.42 * math.cos(a) - GLOW_A.width / 2
    gy = H * 0.42 + H * 0.22 * math.sin(2 * a) - GLOW_A.height / 2
    frame.paste(GLOW_A, (int(gx), int(gy)), GLOW_A)
    gx = W * 0.5 - W * 0.38 * math.cos(a + 1.3) - GLOW_B.width / 2
    gy = H * 0.68 - H * 0.16 * math.sin(a + 1.3) - GLOW_B.height / 2
    frame.paste(GLOW_B, (int(gx), int(gy)), GLOW_B)
    frame.paste(static, (0, 0), static)

    p = 0.5 - 0.5 * math.cos(2 * a)  # two soft pulses per loop
    h = halo_rgb.copy()
    h.putalpha(halo.point(lambda v, p=p: int(v * 0.85 * p)))
    frame.paste(h, (px - 40, py - 40), h)
    size = int(round(TILE * (1.0 + 0.07 * p)))
    tile = shrink(pulse_src, (size, size))
    off = (TILE - size) // 2
    frame.paste(tile, (px + off, py + off), tile)

    enc.stdin.write(frame.tobytes())
enc.stdin.close()
if enc.wait() != 0:
    sys.exit("ffmpeg failed")
'@

$tmp = Join-Path ([IO.Path]::GetTempPath()) "airplay-make-demo-$PID"
try {
    New-Item -ItemType Directory -Force $tmp | Out-Null
    $script = Join-Path $tmp 'make_demo.py'
    [IO.File]::WriteAllText($script, $py, (New-Object Text.UTF8Encoding $false))
    $venv = Join-Path $tmp 'venv'
    & uv venv --quiet $venv
    if ($LASTEXITCODE -ne 0) { throw 'uv venv failed' }
    $python = Join-Path $venv 'Scripts\python.exe'
    & uv pip install --quiet --python $python pillow
    if ($LASTEXITCODE -ne 0) { throw 'uv pip install pillow failed' }
    & $python -I $script $Out (Get-Command $Ffmpeg).Source
    if ($LASTEXITCODE -ne 0) { throw 'generating the demo video failed' }
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
$size = (Get-Item $Out).Length
'{0}: {1:N0} bytes' -f $Out, $size
if ($size -gt 2MB) { Write-Warning 'demo video is larger than 2 MB' }
