"""Cleans VectorCraft's Image Trace of the sloth into the site's SVG.

The trace (colour, abutting) has the flat magenta background as its own shapes and
the dark outline as many separate shapes. This replaces both with one silhouette in
the outline colour: the artboard minus the background regions (even-odd), sent to the
back. That's the sloth's exact outline, and it also hides the hairline seams browsers
draw between abutting shapes. Coordinates are halved (the trace was made at 2x).

    python scripts/clean-trace.py trace-raw.svg docs/img/sloth.svg
"""
import re
from collections import Counter
import sys
from pathlib import Path

KEY = "#ff00ff"
OUTLINE = "#291407"
SCALE = 0.5


def fmt(v: float) -> str:
    s = f"{v:.1f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def scale_d(d: str) -> str:
    return re.sub(r"-?\d+(?:\.\d+)?", lambda m: fmt(float(m.group()) * SCALE), d)


# Flattening to 14 colours merged the nose into the eye-patch brown; the art's nose is
# darker. (box in the 2x trace's coordinates, colour sampled from the original art)
NOSE_BOX, NOSE_FROM, NOSE_TO = (740, 550, 910, 680), "#6b3e21", "#50301f"


def touch_up(d: str, fill: str) -> str:
    if fill != NOSE_FROM:
        return fill
    n = [float(v) for v in re.findall(r"-?\d+(?:\.\d+)?", d)]
    xs, ys = n[0::2], n[1::2]
    x0, y0, x1, y1 = NOSE_BOX
    inside = x0 <= min(xs) and max(xs) <= x1 and y0 <= min(ys) and max(ys) <= y1
    return NOSE_TO if inside else fill


def main(src: str, dst: str):
    s = Path(src).read_text(encoding="utf-8")
    vb = [float(v) for v in re.search(r'viewBox="([^"]+)"', s).group(1).split()]
    paths = re.findall(r'<path d="([^"]+)" fill="([^"]+)"\s*/>', s)
    if len(paths) != s.count("<path"):
        raise SystemExit("unexpected path markup; update the pattern")

    keyed = [d for d, f in paths if f == KEY]
    w, h = vb[2], vb[3]
    # A frame a little past the artboard, so background shapes that overshoot it still cut.
    frame = f"M-30 -30H{w + 30}V{h + 30}H-30Z"
    silhouette = scale_d(frame + "".join(keyed))

    # Colours on only a path or two are edge-blend artifacts (a red where outline met
    # magenta): fold each into the nearest real palette colour.
    counts = Counter(f for _, f in paths)
    palette = [c for c, n in counts.items() if n >= 5]
    rgb = lambda c: tuple(int(c[i : i + 2], 16) for i in (1, 3, 5))
    nearest = lambda c: min(palette, key=lambda p: sum((a - b) ** 2 for a, b in zip(rgb(c), rgb(p))))
    paths = [(d, f if f in palette else nearest(f)) for d, f in paths]
    paths = [(d, touch_up(d, f)) for d, f in paths]
    kept = [(d, f) for d, f in paths if f not in (KEY, OUTLINE)]

    body = [f'<path fill="{OUTLINE}" fill-rule="evenodd" d="{silhouette}"/>']
    body += [f'<path fill="{f}" d="{scale_d(d)}"/>' for d, f in kept]
    out = (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {fmt(w * SCALE)} {fmt(h * SCALE)}">'
        "<title>SlothSearch's sloth</title>" + "".join(body) + "</svg>"
    )
    Path(dst).parent.mkdir(parents=True, exist_ok=True)
    Path(dst).write_text(out, encoding="utf-8")
    print(f"{len(kept) + 1} paths ({len(paths)} traced), {len(out) / 1024:.0f} KB -> {dst}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
