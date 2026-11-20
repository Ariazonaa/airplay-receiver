# Erzeugt das App-Icon (Variante B: Monitor mit Handy davor, Wellen dazwischen).
# Aufruf aus dem Repo-Wurzelverzeichnis:
#   uv run --no-project --with pillow python scripts/make-icon.py [--preview <pfad.png>]
# Schreibt assets/icon/airplay-256.png und assets/icon/airplay.ico (je Größe ein eigenes Bild).
# Mit --preview zusätzlich ein Vorschau-Blatt der kleinen Größen (nicht eingecheckt).
import argparse
import io
import struct
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "assets" / "icon"
S = 1024  # Arbeitsgröße der großen Zeichnung
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
SMALL_MAX = 32  # bis hier die vereinfachte, pixelgenaue Zeichnung

TEAL_A = (16, 122, 134)  # oben
TEAL_B = (8, 74, 92)  # unten
WHITE = (255, 255, 255, 255)


def gradient(size):
    """Senkrechter Verlauf TEAL_A -> TEAL_B."""
    grad = Image.new("RGBA", (size, size))
    gd = ImageDraw.Draw(grad)
    for y in range(size):
        t = y / (size - 1)
        c = tuple(round(TEAL_A[i] * (1 - t) + TEAL_B[i] * t) for i in range(3)) + (255,)
        gd.line([(0, y), (size, y)], fill=c)
    return grad


def background(size, margin, radius):
    """Abgerundetes Quadrat mit Verlauf; Maske 4x überabgetastet für glatte Ecken."""
    k = 4
    mask = Image.new("L", (size * k, size * k), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [margin * k, margin * k, (size - margin) * k - 1, (size - margin) * k - 1],
        radius=radius * k,
        fill=255,
    )
    mask = mask.resize((size, size), Image.LANCZOS)
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    img.paste(gradient(size), (0, 0), mask)
    return img


def arcs(d, cx, cy, radii, w, start, end):
    for r in radii:
        d.arc([cx - r, cy - r, cx + r, cy + r], start=start, end=end, fill=WHITE, width=w)


def large_icon():
    """Volle Zeichnung in 1024 px (Formen wie der freigegebene Entwurf)."""
    img = background(S, 40, 210)
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([170, 230, 860, 680], radius=60, outline=WHITE, width=46)
    d.rounded_rectangle([440, 680, 590, 760], radius=10, fill=WHITE)
    d.rounded_rectangle([360, 750, 670, 800], radius=25, fill=WHITE)
    # Handy vorne links, mit Hintergrundfarbe gefüllt, damit es den Monitor überdeckt
    d.rounded_rectangle([110, 470, 330, 860], radius=48, outline=WHITE, width=40, fill=TEAL_B + (255,))
    arcs(d, 330, 470, [110, 200], 40, -60, 0)
    return img


def small_icon(n):
    """Vereinfachte Zeichnung für 16-32 px: keine Wellen, dicke Linien, ganze Pixel.

    Entworfen auf einem 16er-Raster (Koordinaten inklusive) und für größere Maße
    auf das Pixelraster gerundet; Striche sind nie dünner als 2 px.
    """
    img = background(n, 0, round(n * 0.22))
    d = ImageDraw.Draw(img)
    f = n / 16

    def p(v):
        return round(v * f)

    def rect(x0, y0, x1, y1, fill=None, outline=None, width=0):
        # x1/y1 sind exklusive Rasterkanten
        d.rectangle([p(x0), p(y0), p(x1) - 1, p(y1) - 1], fill=fill, outline=outline, width=width)

    w = max(2, p(1.0))
    rect(3, 2, 15, 10, outline=WHITE, width=w)  # Monitor
    rect(8, 10, 10, 12, fill=WHITE)  # Hals
    rect(7, 12, 13, 14, fill=WHITE)  # Fuß
    rect(1, 6, 6, 15, fill=TEAL_B + (255,), outline=WHITE, width=w)  # Handy
    return img


def build(n):
    if n <= SMALL_MAX:
        return small_icon(n)
    return large_icon().resize((n, n), Image.LANCZOS)


def png_bytes(img):
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def write_ico(path, images):
    """ICO mit PNG-Einträgen (ab Vista überall lesbar), ein Bild pro Größe."""
    blobs = [(img.width, png_bytes(img)) for img in images]
    head = struct.pack("<HHH", 0, 1, len(blobs))
    offset = 6 + 16 * len(blobs)
    entries = b""
    for n, data in blobs:
        dim = 0 if n >= 256 else n
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    path.write_bytes(head + entries + b"".join(d for _, d in blobs))


def main():
    parser = argparse.ArgumentParser(description="Erzeugt das App-Icon (ICO + 256-px-PNG).")
    parser.add_argument("--preview", type=Path, help="zusätzlich ein Vorschau-Blatt als PNG hierhin schreiben")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    images = [build(n) for n in SIZES]
    (OUT / "airplay-256.png").write_bytes(png_bytes(images[-1]))
    write_ico(OUT / "airplay.ico", images)
    if args.preview:
        # Vorschau-Blatt: die kleinen Größen 1:1 und 4x vergrößert (Pixel sichtbar)
        sheet = Image.new("RGBA", (sum(n * 5 + 12 for n in SIZES[:4]) + 40, 400), (240, 240, 240, 255))
        x = 10
        for img in images[:4]:
            sheet.alpha_composite(img, (x, 10))
            sheet.alpha_composite(img.resize((img.width * 4, img.height * 4), Image.NEAREST), (x, 60))
            x += img.width * 5 + 12
        args.preview.parent.mkdir(parents=True, exist_ok=True)
        sheet.convert("RGB").save(args.preview)
        print("Vorschau:", args.preview)
    print("ok:", OUT)


if __name__ == "__main__":
    main()
