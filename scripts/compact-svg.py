"""Shrinks path data in an SVG: absolute commands become relative ones, numbers keep one
decimal, and separators are dropped where a minus sign or a new command already
separates values. Handles M, L, H, V, C, S, Q, T and Z (what VectorCraft exports for
traced art); anything with an arc is left untouched.

    python scripts/compact-svg.py in.svg out.svg [--clip]
"""
import re
import sys
from pathlib import Path

ARGS = {"M": 2, "L": 2, "H": 1, "V": 1, "C": 6, "S": 4, "Q": 4, "T": 2, "Z": 0}


def num(v: float) -> str:
    s = f"{v:.1f}"
    if s.endswith(".0"):
        s = s[:-2]
    if s.startswith("0."):
        s = s[1:]
    elif s.startswith("-0."):
        s = "-" + s[2:]
    return "0" if s in ("-0", "") else s


def join(values):
    out = ""
    for v in values:
        t = num(v)
        # A leading '-' or '.' after a number without '.' already separates values.
        if out and not (t.startswith("-") or (t.startswith(".") and "." in out.split(" ")[-1].split("-")[-1])):
            out += " "
        out += t
    return out


def compact(d: str) -> str:
    tokens = re.findall(r"[A-Za-z]|-?(?:\d+\.?\d*|\.\d+)", d)
    if any(t in "Aa" for t in tokens):
        return d
    out = []
    cx = cy = sx = sy = 0.0
    i = 0
    cmd = None
    while i < len(tokens):
        t = tokens[i]
        if t.isalpha():
            cmd = t
            i += 1
            if cmd in "Zz":
                out.append("z")
                cx, cy = sx, sy
                continue
        if cmd is None:
            return d
        up = cmd.upper()
        n = ARGS[up]
        # Absolute points are rounded first, so relative steps never accumulate drift.
        vals = [float(v) for v in tokens[i : i + n]]
        i += n
        rel = cmd.islower()
        if up == "H":
            x = round(vals[0] + (cx if rel else 0), 1)
            out.append("h" + join([x - cx]))
            cx = x
        elif up == "V":
            y = round(vals[0] + (cy if rel else 0), 1)
            out.append("v" + join([y - cy]))
            cy = y
        else:
            pts = []
            for k in range(0, n, 2):
                px, py = vals[k], vals[k + 1]
                if rel:
                    px, py = px + cx, py + cy
                pts.append((round(px, 1), round(py, 1)))
            out.append(up.lower() + join([c for p in pts for c in (p[0] - cx, p[1] - cy)]))
            cx, cy = pts[-1]
            if up == "M":
                sx, sy = cx, cy
                cmd = "l" if rel else "L"  # extra pairs after M are line-tos
    return "".join(out)


def clip_to_viewbox(s: str) -> str:
    """Wrap the art in a clip to its viewBox: shapes that overshoot it would otherwise
    show wherever the SVG is letterboxed."""
    x, y, w, h = re.search(r'viewBox="([^"]+)"', s).group(1).split()
    open_end = s.index(">", s.index("<svg")) + 1
    title_end = s.find("</title>") + len("</title>") if "</title>" in s else open_end
    clip = f'<clipPath id="vb"><rect x="{x}" y="{y}" width="{w}" height="{h}"/></clipPath><g clip-path="url(#vb)">'
    close = s.rindex("</svg>")
    return s[:title_end] + clip + s[title_end:close] + "</g></svg>"


def main(src: str, dst: str, clip: bool = False):
    s = Path(src).read_text(encoding="utf-8")
    before = len(s)
    s = re.sub(r' d="([^"]+)"', lambda m: f' d="{compact(m.group(1))}"', s)
    if clip:
        s = clip_to_viewbox(s)
    Path(dst).write_text(s, encoding="utf-8")
    print(f"{before / 1024:.0f} KB -> {len(s) / 1024:.0f} KB")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], clip="--clip" in sys.argv[3:])
