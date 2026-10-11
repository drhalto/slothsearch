"""Builds SlothSearch's brand assets from the generated sloth art.

Inputs (gpt-image-2 renders on flat #FF00FF, from codex-image):
    assets/brand/sloth-mascot.png   full-body mascot
    assets/brand/sloth-icon.png     head + lens close-up

Outputs:
    assets/brand/sloth-*-transparent.png   keyed, edges cleaned
    assets/slothsearch.ico                 app + tray icon (16-256 px)
    assets/sloth-head-64.rgba              footer mark, raw straight-alpha RGBA 64x64
    assets/sloth-mascot-256.rgba           empty-state art, raw straight-alpha RGBA 256x256

    python scripts/make-brand.py
"""
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
BRAND = ROOT / "assets" / "brand"
KEY = (255, 0, 255)
NEAR, FAR = 60.0, 150.0  # distance to magenta: <= NEAR clear, >= FAR solid


def keyness(p):
    return (p[0] + p[2]) / 2 - p[1]


def key_out(src: Path) -> Image.Image:
    """Alpha from distance to pure magenta (so the indigo accents, which are far from
    it, stay untouched), then recolor the thin edge band from its solid neighbors so
    no pink halo is left behind."""
    im = Image.open(src).convert("RGB")
    w, h = im.size
    px = im.load()
    out = Image.new("RGBA", (w, h))
    op = out.load()
    for y in range(h):
        for x in range(w):
            r, g, b = px[x, y]
            d = ((r - KEY[0]) ** 2 + (g - KEY[1]) ** 2 + (b - KEY[2]) ** 2) ** 0.5
            a = 0 if d <= NEAR else 255 if d >= FAR else round((d - NEAR) / (FAR - NEAR) * 255)
            op[x, y] = (r, g, b, a)

    def tinted(x, y):
        r, g, b, a = op[x, y]
        return a < 255 or keyness((r, g, b)) > 30

    band = []
    for y in range(h):
        for x in range(w):
            a = op[x, y][3]
            if a == 0:
                continue
            if a < 255:
                band.append((x, y))
                continue
            # Solid pixels touching the background that still lean magenta.
            if keyness(op[x, y]) > 30 and any(
                0 <= x + dx < w and 0 <= y + dy < h and op[x + dx, y + dy][3] < 200
                for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1))
            ):
                band.append((x, y))
    fixed = {}
    for x, y in band:
        acc = [0, 0, 0, 0]
        for dy in range(-3, 4):
            for dx in range(-3, 4):
                nx, ny = x + dx, y + dy
                if 0 <= nx < w and 0 <= ny < h and not tinted(nx, ny):
                    r, g, b, _ = op[nx, ny]
                    acc[0] += r
                    acc[1] += g
                    acc[2] += b
                    acc[3] += 1
        if acc[3]:
            fixed[(x, y)] = (acc[0] // acc[3], acc[1] // acc[3], acc[2] // acc[3])
        else:
            fixed[(x, y)] = (40, 26, 20)  # the art's outline brown
    for (x, y), rgb in fixed.items():
        op[x, y] = rgb + (op[x, y][3],)
    return out


def square(im: Image.Image, pad: float = 0.02) -> Image.Image:
    """Trim transparent margins, then center on a square canvas."""
    im = im.crop(im.getbbox())
    side = round(max(im.size) * (1 + pad * 2))
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.alpha_composite(im, ((side - im.width) // 2, (side - im.height) // 2))
    return canvas


def write_rgba(im: Image.Image, size: int, path: Path):
    path.write_bytes(im.resize((size, size), Image.LANCZOS).tobytes())


def main():
    mascot = key_out(BRAND / "sloth-mascot.png")
    head = key_out(BRAND / "sloth-icon.png")
    mascot.save(BRAND / "sloth-mascot-transparent.png")
    head.save(BRAND / "sloth-icon-transparent.png")

    head_sq = square(head)
    # Small sizes drop the shoulders so the face and lens stay legible at 16-24 px.
    face_sq = square(head.crop((60, 40, 990, 800)))
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    frames = [(face_sq if s <= 48 else head_sq).resize((s, s), Image.LANCZOS) for s in sizes]
    frames[-1].save(ROOT / "assets" / "slothsearch.ico", sizes=[(s, s) for s in sizes], append_images=frames[:-1])

    write_rgba(face_sq, 64, ROOT / "assets" / "sloth-head-64.rgba")
    write_rgba(square(mascot), 256, ROOT / "assets" / "sloth-mascot-256.rgba")
    print("brand assets written")


if __name__ == "__main__":
    main()
