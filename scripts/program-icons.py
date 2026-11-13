#!/usr/bin/env python3
"""Generate Rex Rune's program icons from Superlogical's Rex icon set.

Usage: scripts/program-icons.py <icon set folder>

The folder is a copy of https://icons.superlogical.com: `manifest.json` (per icon its name,
slug, tile layers under `svg/`, glyph `glyphs/<slug>.svg`, glyph colour and the glyph's box on
the 73x60 tile) and the files it names. The tiles are written for a browser: CSS
`color(display-p3 …)` and `color-mix(in oklch, …)` colours, 32- to 50-stop gradients and a glyph
that is a CSS mask filled with the glyph colour. Rex draws icons with resvg, which knows none of
that, so this writes, per icon:

- `resources/program-icons/tiles/<slug>.svg`: the tile with the glyph composited in through an
  alpha mask, on a square 60-unit canvas centred on the tile, sRGB only.
- `resources/program-icons/glyphs/<slug>.svg`: the glyph alone on a square canvas, for drawing
  as a one-colour silhouette (`El::icon_one_color`, the CSS mask's meaning) in the chrome's ink.

and `src/app/program_icons/catalog.rs`, which includes them, with each icon's tint for coloured
tab icons. Colours are converted from Display P3 and mixed in OKLCH as CSS does, then clamped to
sRGB; gradient stops that linear interpolation reproduces within a 255th are dropped and
coordinates are rounded to a 4000th of the drawing. The nine tiles drawn with a blurred
mesh gradient (hundreds of patches) get a coarse grid of the patches' mean colours under the same
blur instead, and the one bitmap glyph (Cua) is traced to rectangles, since Rex's resvg is built
without raster images. Needs Pillow for that bitmap. Whitespace between elements is dropped, and
the output replaces the old only once every icon has been written.
"""

import base64
import io
import json
import math
import os
import re
import shutil
import sys
import urllib.parse
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "resources" / "program-icons"
CATALOG = ROOT / "src" / "app" / "program_icons" / "catalog.rs"

SVG = "http://www.w3.org/2000/svg"
XLINK = "http://www.w3.org/1999/xlink"
ET.register_namespace("", SVG)
ET.register_namespace("xlink", XLINK)

# The tile's square canvas: 60 units wide, centred on the 73x60 tile's rounded rectangle.
TILE_VIEW = 60.0
TILE_SHIFT = (73.0 - TILE_VIEW) / 2

# --- Colour -------------------------------------------------------------------------------------


def srgb_decode(c):
    return c / 12.92 if abs(c) <= 0.04045 else math.copysign(((abs(c) + 0.055) / 1.055) ** 2.4, c)


def srgb_encode(c):
    return 12.92 * c if abs(c) <= 0.0031308 else math.copysign(1.055 * abs(c) ** (1 / 2.4) - 0.055, c)


def mat(m, v):
    return [sum(m[i][j] * v[j] for j in range(3)) for i in range(3)]


P3_TO_XYZ = [
    [0.4865709486482162, 0.26566769316909306, 0.1982172852343625],
    [0.2289745640697488, 0.6917385218365064, 0.079286914093745],
    [0.0, 0.04511338185890264, 1.043944368900976],
]
XYZ_TO_SRGB = [
    [3.2409699419045226, -1.537383177570094, -0.4986107602930034],
    [-0.9692436362808796, 1.8759675015077202, 0.04155505740717559],
    [0.05563007969699366, -0.20397695888897652, 1.0569715142428786],
]
SRGB_TO_LMS = [
    [0.4122214708, 0.5363325363, 0.0514459929],
    [0.2119034982, 0.6806995451, 0.1073969566],
    [0.0883024619, 0.2817188376, 0.6299787005],
]
LMS_TO_OKLAB = [
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
]
OKLAB_TO_LMS = [
    [1.0, 0.3963377774, 0.2158037573],
    [1.0, -0.1055613458, -0.0638541728],
    [1.0, -0.0894841775, -1.2914855480],
]
LMS_TO_SRGB = [
    [4.0767416621, -3.3077115913, 0.2309699292],
    [-1.2684380046, 2.6097574011, -0.3413193965],
    [-0.0041960863, -0.7034186147, 1.7076147010],
]


def p3_to_srgb(rgb):
    """Display P3 (encoded) to sRGB (encoded, possibly out of gamut)."""
    return [srgb_encode(c) for c in mat(XYZ_TO_SRGB, mat(P3_TO_XYZ, [srgb_decode(c) for c in rgb]))]


def to_oklch(rgb):
    lms = [math.copysign(abs(c) ** (1 / 3), c) for c in mat(SRGB_TO_LMS, [srgb_decode(c) for c in rgb])]
    l, a, b = mat(LMS_TO_OKLAB, lms)
    return l, math.hypot(a, b), math.degrees(math.atan2(b, a)) % 360


def from_oklch(lch):
    l, c, h = lch
    lab = [l, c * math.cos(math.radians(h)), c * math.sin(math.radians(h))]
    return [srgb_encode(v) for v in mat(LMS_TO_SRGB, [v**3 for v in mat(OKLAB_TO_LMS, lab)])]


def mix_oklch(a, b, wa, wb):
    """CSS `color-mix(in oklch, a wa, b wb)` with `shorter` hue interpolation."""
    total = wa + wb
    wa, wb = wa / total, wb / total
    la, ca, ha = to_oklch(a[:3])
    lb, cb, hb = to_oklch(b[:3])
    # A colour without chroma has no hue (CSS's "powerless" hue): it takes the other's.
    if ca < 1e-4:
        ha = hb
    if cb < 1e-4:
        hb = ha
    d = (hb - ha + 180) % 360 - 180
    aa, ab = a[3], b[3]
    alpha = aa * wa + ab * wb
    # Premultiplied interpolation of the non-hue components, as CSS specifies.
    if alpha > 0:
        l = (la * aa * wa + lb * ab * wb) / alpha
        c = (ca * aa * wa + cb * ab * wb) / alpha
    else:
        l, c = la * wa + lb * wb, ca * wa + cb * wb
    return from_oklch((l, c, ha + d * wb)) + [alpha]


NAMED = {"white": [1, 1, 1, 1], "black": [0, 0, 0, 1], "transparent": [0, 0, 0, 0]}


def split_args(s):
    """Top-level comma-separated parts of `s`."""
    parts, depth, start = [], 0, 0
    for i, ch in enumerate(s):
        depth += {"(": 1, ")": -1}.get(ch, 0)
        if ch == "," and depth == 0:
            parts.append(s[start:i])
            start = i + 1
    return [p.strip() for p in parts + [s[start:]]]


def parse_color(s):
    """`s` as sRGB [r, g, b, a], or None for none, currentColor and paint servers."""
    s = s.strip()
    low = s.lower()
    if low in NAMED:
        return list(NAMED[low])
    if low in ("none", "currentcolor") or low.startswith("url("):
        return None
    if s.startswith("#"):
        h = s[1:]
        if len(h) in (3, 4):
            h = "".join(ch * 2 for ch in h)
        v = [int(h[i : i + 2], 16) / 255 for i in range(0, len(h), 2)]
        return v + [1.0] if len(v) == 3 else v
    m = re.fullmatch(r"rgba?\((.*)\)", s)
    if m:
        v = [float(x.strip().rstrip("%")) for x in re.split(r"[,\s/]+", m.group(1).strip()) if x.strip()]
        return [v[0] / 255, v[1] / 255, v[2] / 255, v[3] if len(v) > 3 else 1.0]
    m = re.fullmatch(r"color\(\s*display-p3\s+([^)]*)\)", s)
    if m:
        parts = m.group(1).split("/")
        rgb = [float(x) for x in parts[0].split()]
        alpha = float(parts[1]) if len(parts) > 1 else 1.0
        return p3_to_srgb(rgb) + [alpha]
    m = re.fullmatch(r"color-mix\(\s*in oklch\s*,(.*)\)", s, re.S)
    if m:
        (a, pa), (b, pb) = [weighted(p) for p in split_args(m.group(1))]
        if pa is None and pb is None:
            pa = pb = 50.0
        pa = 100 - pb if pa is None else pa
        pb = 100 - pa if pb is None else pb
        return mix_oklch(parse_color(a), parse_color(b), pa, pb)
    raise ValueError(f"unknown colour {s!r}")


def weighted(part):
    m = re.fullmatch(r"(.*\S)\s+([\d.]+)%", part, re.S)
    return (m.group(1), float(m.group(2))) if m else (part, None)


def clamp(rgb):
    return [min(1.0, max(0.0, c)) for c in rgb]


def hex_of(rgb):
    return "#" + "".join(f"{round(c * 255):02x}" for c in clamp(rgb))


def luminance(rgb):
    lin = [srgb_decode(c) for c in clamp(rgb)]
    return 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]


# --- Numbers and paths --------------------------------------------------------------------------

NUM = re.compile(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")


def fmt(v, decimals):
    s = f"{v:.{max(decimals, 0)}f}"
    if "." in s:
        s = s.rstrip("0").rstrip(".")
    if s in ("-0", ""):
        s = "0"
    if s.startswith("0."):
        s = s[1:]
    elif s.startswith("-0."):
        s = "-" + s[2:]
    return s


def join_numbers(tokens):
    out = ""
    prev = ""
    for t in tokens:
        if t.isalpha():
            out += t
        elif prev and not prev.isalpha() and not t.startswith("-") and not (t.startswith(".") and "." in prev):
            out += " " + t
        else:
            out += t
        prev = t
    return out


def round_path(d, decimals):
    tokens, i, cmd, arg = [], 0, "", 0
    while i < len(d):
        ch = d[i]
        if ch in " ,\t\r\n":
            i += 1
        elif ch.isalpha() and ch not in "eE":
            cmd, arg = ch, 0
            tokens.append(ch)
            i += 1
        elif cmd in "Aa" and arg % 7 in (3, 4):
            tokens.append(ch)
            arg += 1
            i += 1
        else:
            m = NUM.match(d, i)
            if not m:
                raise ValueError(f"bad path data at {d[i:i + 20]!r}")
            tokens.append(fmt(float(m.group()), decimals))
            arg += 1
            i = m.end()
    return join_numbers(tokens)


def round_list(s, decimals):
    return join_numbers([fmt(float(n), decimals) for n in NUM.findall(s)])


def round_in(s, decimals):
    return NUM.sub(lambda m: fmt(float(m.group()), decimals), s)


def transform_scale(t):
    """The area scale of an SVG transform list, as a length factor."""
    scale = 1.0
    for name, args in re.findall(r"(\w+)\s*\(([^)]*)\)", t or ""):
        v = [float(x) for x in NUM.findall(args)]
        if name == "scale" and v:
            scale *= math.sqrt(abs(v[0] * (v[1] if len(v) > 1 else v[0])))
        elif name == "matrix" and len(v) == 6:
            scale *= math.sqrt(abs(v[0] * v[3] - v[1] * v[2]))
    return scale or 1.0


# --- SVG cleaning -------------------------------------------------------------------------------

PRESENTATION = {
    "fill", "fill-opacity", "fill-rule", "stroke", "stroke-width", "stroke-opacity",
    "stroke-linecap", "stroke-linejoin", "stroke-miterlimit", "stroke-dasharray",
    "stroke-dashoffset", "opacity", "stop-color", "stop-opacity", "clip-rule", "display",
    "visibility", "mask", "clip-path", "filter", "flood-color", "flood-opacity", "mix-blend-mode",
}
COLOR_OPACITY = {"fill": "fill-opacity", "stroke": "stroke-opacity", "stop-color": "stop-opacity", "flood-color": "flood-opacity"}
DROP = {"title", "desc", "metadata"}
GEOMETRY = {"x", "y", "width", "height", "cx", "cy", "r", "rx", "ry", "x1", "y1", "x2", "y2", "fx", "fy", "stroke-width", "stroke-dashoffset"}


def local(tag):
    return tag.rsplit("}", 1)[-1]


def clean(el, decimals, prefix):
    """`el` and its descendants cleaned in place for resvg: sRGB colours, style attributes as
    presentation attributes, ids prefixed, numbers rounded to `decimals` places."""
    for child in list(el):
        if not isinstance(child.tag, str) or local(child.tag) in DROP or child.tag.startswith("{http://www.inkscape") or child.tag.startswith("{http://sodipodi"):
            el.remove(child)
    style = el.attrib.pop("style", "")
    for decl in style.split(";"):
        if ":" in decl:
            k, v = (x.strip() for x in decl.split(":", 1))
            if k in PRESENTATION:
                el.set(k, v)
    for k in list(el.attrib):
        if k.startswith("{") and not k.startswith("{" + XLINK):
            del el.attrib[k]
    if el.get("mix-blend-mode") == "normal":
        del el.attrib["mix-blend-mode"]
    for k in ("isolation", "class", "aria-hidden", "version", "data-name"):
        el.attrib.pop(k, None)
    for k, opacity in COLOR_OPACITY.items():
        v = el.get(k)
        if v is None:
            continue
        c = parse_color(v)
        if c is None:
            if v.strip().lower() == "currentcolor":
                el.set(k, "#000000")
            continue
        el.set(k, hex_of(c[:3]))
        if c[3] < 1:
            el.set(opacity, fmt(float(el.get(opacity, "1")) * c[3], 3))
    scale = transform_scale(el.get("transform")) * transform_scale(el.get("gradientTransform"))
    own = decimals + max(0, math.ceil(math.log10(1 / scale))) if scale < 1 else decimals
    for k in list(el.attrib):
        v = el.attrib[k]
        if k == "id":
            el.set(k, prefix + v)
        elif k in ("href", "{%s}href" % XLINK) and v.startswith("#"):
            el.set(k, "#" + prefix + v[1:])
        elif "url(#" in v:
            el.set(k, v.replace("url(#", "url(#" + prefix))
        elif k == "d":
            el.set(k, round_path(v, own))
        elif k == "points":
            el.set(k, round_list(v, own))
        elif k in ("transform", "gradientTransform", "patternTransform"):
            el.set(k, round_in(v, 4))
        elif k in GEOMETRY and NUM.fullmatch(v.strip()):
            el.set(k, fmt(float(v), own))
        elif k == "offset":
            el.set(k, fmt(float(v.rstrip("%")) / (100 if v.endswith("%") else 1), 4))
    for child in el:
        clean(child, own, prefix)
    if local(el.tag) in ("linearGradient", "radialGradient"):
        simplify_stops(el)


def simplify_stops(grad):
    stops = [s for s in grad if local(s.tag) == "stop"]
    if len(stops) < 3:
        return

    def value(s):
        c = parse_color(s.get("stop-color", "#000000")) or [0, 0, 0, 1]
        return [float(s.get("offset", "0"))] + clamp(c[:3]) + [float(s.get("stop-opacity", "1"))]

    vals = [value(s) for s in stops]
    keep = {0, len(vals) - 1}

    def rdp(a, b):
        worst, at = 0.0, None
        for i in range(a + 1, b):
            t = 0 if vals[b][0] == vals[a][0] else (vals[i][0] - vals[a][0]) / (vals[b][0] - vals[a][0])
            err = max(abs(vals[a][k] + (vals[b][k] - vals[a][k]) * t - vals[i][k]) for k in range(1, 5))
            if err > worst:
                worst, at = err, i
        if at is not None and worst > 1 / 255:
            keep.add(at)
            rdp(a, at)
            rdp(at, b)

    rdp(0, len(vals) - 1)
    for i, s in enumerate(stops):
        if i not in keep:
            grad.remove(s)


def serialize(el):
    for e in el.iter():
        if e.text is not None and not e.text.strip():
            e.text = None
        if e.tail is not None and not e.tail.strip():
            e.tail = None
    s = ET.tostring(el, encoding="unicode")
    return re.sub(r' xmlns(:xlink)?="[^"]*"', "", s)


def document(view, inner, xlink):
    ns = f' xmlns:xlink="{XLINK}"' if xlink else ""
    v = fmt(view, 3)
    return f'<svg xmlns="{SVG}"{ns} viewBox="0 0 {v} {v}" width="{v}" height="{v}">{inner}</svg>\n'


def view_box(root):
    vb = root.get("viewBox")
    if vb:
        return [float(x) for x in NUM.findall(vb)]
    return [0, 0, float(NUM.match(root.get("width")).group()), float(NUM.match(root.get("height")).group())]


def decimals_for(size):
    return max(0, math.ceil(math.log10(4000 / size)))


# --- Bitmap glyphs ------------------------------------------------------------------------------


def trace_images(root, prefix):
    """Every embedded bitmap in `root` replaced by its alpha traced to rectangles."""
    for parent in list(root.iter()):
        for img in [c for c in parent if local(c.tag) == "image"]:
            href = img.get("{%s}href" % XLINK) or img.get("href", "")
            from PIL import Image

            data = base64.b64decode(href.split(",", 1)[1])
            alpha = Image.open(io.BytesIO(data)).convert("RGBA").getchannel("A")
            x, y = float(img.get("x", "0")), float(img.get("y", "0"))
            w, h = float(img.get("width")), float(img.get("height"))
            grid = 128
            cols, rows = (grid, max(1, round(grid * h / w))) if w >= h else (max(1, round(grid * w / h)), grid)
            small = alpha.resize((cols, rows), Image.Resampling.BOX)
            px = small.load()
            cw, ch = w / cols, h / rows
            d = []
            for r in range(rows):
                c = 0
                while c < cols:
                    if px[c, r] < 128:
                        c += 1
                        continue
                    start = c
                    while c < cols and px[c, r] >= 128:
                        c += 1
                    d.append(f"M{fmt(x + start * cw, 2)} {fmt(y + r * ch, 2)}h{fmt((c - start) * cw, 2)}v{fmt(ch, 2)}h{fmt(-(c - start) * cw, 2)}z")
            path = ET.Element(f"{{{SVG}}}path", {"d": "".join(d), "fill": "#000000"})
            if img.get("transform"):
                path.set("transform", img.get("transform"))
            parent.insert(list(parent).index(img), path)
            parent.remove(img)


# --- Mesh gradients -----------------------------------------------------------------------------


def coarsen_meshes(root):
    """Every blurred mesh gradient in `root` (a filtered group of hundreds of one-colour patches)
    replaced by a grid of cells about one and a half blur radii wide, each its patches' mean
    colour, under the same blur: after the blur the two look the same, at a tenth of the size."""
    filters = {}
    for el in root.iter():
        if local(el.tag) == "filter":
            blur = el.find(f"{{{SVG}}}feGaussianBlur")
            if blur is not None:
                filters[el.get("id")] = float(NUM.findall(blur.get("stdDeviation", "1"))[0])
    for g in root.iter():
        ref = re.fullmatch(r"url\(#(.*)\)", g.get("filter", ""))
        paths = [c for c in g if local(c.tag) == "path"]
        if not ref or ref.group(1) not in filters or len(paths) < 50 or len(paths) != len(g):
            continue
        patches = []
        for el in paths:
            style = dict(
                (k.strip(), v.strip()) for k, v in (d.split(":", 1) for d in el.get("style", "").split(";") if ":" in d)
            )
            c = parse_color(style.get("fill") or el.get("fill", "none"))
            nums = [float(n) for n in NUM.findall(el.get("d", ""))]
            xs, ys = nums[0::2], nums[1::2]
            if c is not None and xs:
                patches.append((sum(xs) / len(xs), sum(ys) / len(ys), clamp(c[:3]), min(xs), max(xs), min(ys), max(ys)))
        x0, x1 = min(p[3] for p in patches), max(p[4] for p in patches)
        y0, y1 = min(p[5] for p in patches), max(p[6] for p in patches)
        cell = filters[ref.group(1)] * 1.5
        cols, rows = max(1, round((x1 - x0) / cell)), max(1, round((y1 - y0) / cell))
        bins = {}
        for x, y, c, *_ in patches:
            key = (min(cols - 1, int((x - x0) / (x1 - x0) * cols)), min(rows - 1, int((y - y0) / (y1 - y0) * rows)))
            bins.setdefault(key, []).append(c)
        for el in paths:
            g.remove(el)
        cw, ch = (x1 - x0) / cols, (y1 - y0) / rows
        for (i, j), cs in sorted(bins.items()):
            mean = [sum(c[k] for c in cs) / len(cs) for k in range(3)]
            # Cells overlap by a hair so no seam shows between them before the blur.
            ET.SubElement(
                g,
                f"{{{SVG}}}rect",
                {"x": fmt(x0 + i * cw, 3), "y": fmt(y0 + j * ch, 3), "width": fmt(cw + 0.05, 3), "height": fmt(ch + 0.05, 3), "fill": hex_of(mean)},
            )


# --- Icons --------------------------------------------------------------------------------------


def glyph_root(src):
    root = ET.parse(src).getroot()
    if root.get("viewBox") is None and root.get("width") is None:
        raise ValueError(f"{src}: no size")
    return root


def fit(vb, x, y, w, h):
    """The transform placing viewBox `vb` in the box, centred and contained (CSS `contain`)."""
    k = min(w / vb[2], h / vb[3])
    tx = x + (w - vb[2] * k) / 2 - vb[0] * k
    ty = y + (h - vb[3] * k) / 2 - vb[1] * k
    return f"translate({fmt(tx, 4)} {fmt(ty, 4)}) scale({fmt(k, 6)})"


def glyph_group(root, transform, prefix):
    """The glyph's drawing in a group placed by `transform`, its numbers rounded in its own
    units."""
    vb = view_box(root)
    g = ET.Element(f"{{{SVG}}}g")
    for child in list(root):
        g.append(child)
    for k in ("fill", "stroke", "fill-rule", "style"):
        if root.get(k):
            g.set(k, root.get(k))
    trace_images(g, prefix)
    clean(g, decimals_for(max(vb[2], vb[3])), prefix)
    g.set("transform", transform)
    return g


def layers(src_dir, icon):
    """The glyph layers the icon's cell draws over its tile, in order: each one's box on the
    73x60 tile, what fills it (a colour, or a drawing to clip), its opacity and its mask's
    drawing."""
    slug = icon["slug"]
    html = (src_dir / "svg" / f"{slug}.cell.html").read_text()
    out, inline = [], []
    for m in re.finditer(r'<span style="(position:absolute;[^"]*)">(<svg.*?</svg>)?</span>', html, re.S):
        style = {}
        for decl in re.split(r";(?![^(]*\))", m.group(1)):
            if ":" in decl:
                k, v = decl.split(":", 1)
                style[k.strip()] = v.strip()
        box = [float(style[k].rstrip("px")) for k in ("left", "top", "width", "height")]
        fill = parse_color(style["background"]) if "background" in style else ET.fromstring(m.group(2))
        url = re.match(r"url\('([^']*)'\)", style["mask"]).group(1)
        if url.startswith("data:"):
            meta, data = url.split(",", 1)
            text = base64.b64decode(data).decode() if meta.endswith(";base64") else urllib.parse.unquote(data)
            text = re.sub(r"^<\?xml[^>]*>", "", text.strip())
            inline.append(text)
            mask = ET.fromstring(text)
        else:
            # The page names glyphs by their upstream file; the copy is saved under its slug.
            path = src_dir / url if (src_dir / url).exists() else src_dir / icon["glyph"]
            try:
                mask = glyph_root(path)
            except ET.ParseError:
                mask = truncated(src_dir, slug, path, box, fill, inline)
        out.append((box, fill, float(style.get("opacity", "1")), mask))
    if not out:
        raise ValueError(f"{slug}: no glyph layers")
    return out


def truncated(src_dir, slug, path, box, fill, inline):
    """A mask the copy saved cut short (at the first `)` of its data URL). A drawn layer masked
    by the start of the cell's complete mask is masked by that mask; a layer of one colour is
    traced from the reference render, as the pixels nearer its colour than the other layers' and
    the tile's."""
    start = re.sub(r"^<\?xml[^>]*>", "", path.read_text().strip())
    if not isinstance(fill, list):
        whole = next((t for t in inline if t.startswith(start)), None)
        if whole is None:
            raise ValueError(f"{slug}: {path.name} is cut short and nothing completes it")
        return ET.fromstring(whole)
    from PIL import Image

    ref = Image.open(src_dir / "tiles" / f"{slug}.png").convert("RGB")
    sx, sy = ref.width / 73, ref.height / 60
    px = ref.load()
    x, y, w, h = box
    x0, y0, x1, y1 = round(x * sx), round(y * sy), round((x + w) * sx), round((y + h) * sy)
    lin = lambda c: [v / 255 for v in c]
    corner = lin(px[x0, y0])
    others = [corner] + [c for c in colours_in(src_dir, slug) if c != fill[:3]]
    target = clamp(fill[:3])
    dist = lambda a, b: sum((a[k] - b[k]) ** 2 for k in range(3))
    d = []
    for row in range(y0, y1):
        col = x0
        while col < x1:
            c = lin(px[col, row])
            hit = lambda c: dist(c, target) < min(dist(c, o) for o in others)
            if not hit(c):
                col += 1
                continue
            start_col = col
            while col < x1 and hit(lin(px[col, row])):
                col += 1
            d.append(
                f"M{fmt(start_col / sx - x, 3)} {fmt(row / sy - y, 3)}"
                f"h{fmt((col - start_col) / sx, 3)}v{fmt(1 / sy, 3)}h{fmt(-(col - start_col) / sx, 3)}z"
            )
    root = ET.Element(f"{{{SVG}}}svg", {"viewBox": f"0 0 {fmt(w, 3)} {fmt(h, 3)}"})
    ET.SubElement(root, f"{{{SVG}}}path", {"d": "".join(d), "fill": "#000000"})
    return root


def colours_in(src_dir, slug):
    """The solid colours of the cell's layers."""
    html = (src_dir / "svg" / f"{slug}.cell.html").read_text()
    return [clamp(parse_color(c)[:3]) for c in re.findall(r"background:(color\([^)]*\))", html)]


def write_glyph(out, slug, spans):
    """The masks of every layer, drawn black and centred on a square around their boxes."""
    x0 = min(b[0] for b, *_ in spans)
    y0 = min(b[1] for b, *_ in spans)
    w = max(b[0] + b[2] for b, *_ in spans) - x0
    h = max(b[1] + b[3] for b, *_ in spans) - y0
    side = max(w, h)
    x0 -= (side - w) / 2
    y0 -= (side - h) / 2
    groups = []
    for i, (box, _, _, mask) in enumerate(spans):
        prefix = f"m{i}" if len(spans) > 1 else ""
        groups.append(serialize(glyph_group(mask, fit(view_box(mask), box[0] - x0, box[1] - y0, box[2], box[3]), prefix)))
    inner = "".join(groups)
    if not re.search(r'(fill|stroke|stop-color)="#', inner):
        inner = f'<g fill="#000000">{inner}</g>'
    (out / "glyphs" / f"{slug}.svg").write_text(document(side, inner, "xlink:href" in inner))
    return side


def write_tile(out, icon, src_dir, spans):
    tile = ET.parse(src_dir / icon["files"][0]).getroot()
    coarsen_meshes(tile)
    clean(tile, 2, "t")
    shift = ET.Element(f"{{{SVG}}}g", {"transform": f"translate({fmt(-TILE_SHIFT, 2)} 0)"})
    for child in list(tile):
        shift.append(child)
    defs = ET.SubElement(shift, f"{{{SVG}}}defs")
    colours = []
    for i, ((x, y, w, h), fill, opacity, glyph) in enumerate(spans):
        rect = {"x": fmt(x, 3), "y": fmt(y, 3), "width": fmt(w, 3), "height": fmt(h, 3)}
        mask = ET.SubElement(defs, f"{{{SVG}}}mask", {"id": f"m{i}", "mask-type": "alpha", "maskUnits": "userSpaceOnUse", **rect})
        mask.append(glyph_group(glyph, fit(view_box(glyph), x, y, w, h), f"m{i}"))
        group = ET.SubElement(shift, f"{{{SVG}}}g", {"mask": f"url(#m{i})"})
        if opacity < 1:
            group.set("opacity", fmt(opacity, 3))
        if isinstance(fill, list):
            ET.SubElement(group, f"{{{SVG}}}rect", {**rect, "fill": hex_of(fill[:3])})
            colours.append(fill[:3])
        else:
            coarsen_meshes(fill)
            layer = ET.SubElement(group, f"{{{SVG}}}g", {"transform": f"translate({fmt(x, 3)} {fmt(y, 3)})"})
            for child in list(fill):
                layer.append(child)
            clean(layer, 3, f"l{i}")
            colours.extend(stop_colours(layer))
    inner = serialize(shift)
    (out / "tiles" / f"{icon['slug']}.svg").write_text(document(TILE_VIEW, inner, "xlink:href" in inner))
    return [base_color(tile)] + colours


def stop_colours(el):
    """The colours a drawn layer paints with: its gradient stops and solid fills."""
    out = []
    for e in el.iter():
        for k in ("stop-color", "fill"):
            c = parse_color(e.get(k, "none"))
            if c is not None:
                out.append(c[:3])
    return out or [[0.5, 0.5, 0.5]]


def base_color(tile):
    """The tile's own colour: its first solid fill, or its base gradient's mean."""
    ids = {el.get("id"): el for el in tile.iter() if el.get("id")}
    for el in tile.iter():
        if local(el.tag) != "rect":
            continue
        fill = el.get("fill", "")
        c = parse_color(fill) if not fill.startswith("url(") else None
        if c:
            return c[:3]
        grad = ids.get(fill[5:-1]) if fill.startswith("url(#") else None
        if grad is not None:
            cs = [parse_color(s.get("stop-color")) for s in grad if local(s.tag) == "stop"]
            return [sum(c[k] for c in cs) / len(cs) for k in range(3)]
    return [0.5, 0.5, 0.5]


def tint(colours):
    """A colour for the glyph on Rex's dark chrome: the hue of the most colourful of the tile's
    own colour and the glyph's, at the OKLCH lightness that gives it a relative luminance of
    0.65 (over 3:1 on the lightest tab surface), brought into sRGB by losing chroma; a light
    grey for an icon without colour."""
    _, c, h = max((to_oklch(clamp(c)) for c in colours), key=lambda lch: lch[1])
    if c < 0.03:
        return 0xD4D4D8
    c = min(c, 0.2)
    l = 0.8
    while True:
        while c > 0 and any(v < -1e-4 or v > 1 + 1e-4 for v in from_oklch((l, c, h))):
            c -= 0.002
        rgb = clamp(from_oklch((l, max(c, 0), h)))
        if luminance(rgb) >= 0.65 or l >= 1:
            return int(hex_of(rgb)[1:], 16)
        l += 0.005


def rust_f32(v):
    s = fmt(v, 3)
    s = ("-0" + s[2:]) if s.startswith("-.") else ("0" + s) if s.startswith(".") else s
    return s if "." in s else s + ".0"


def rust_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    src_dir = Path(sys.argv[1]).expanduser()
    icons = json.loads((src_dir / "manifest.json").read_text())
    staging = OUT / ".new"
    shutil.rmtree(staging, ignore_errors=True)
    for sub in ("tiles", "glyphs"):
        (staging / sub).mkdir(parents=True)
    rows = []
    for icon in sorted(icons, key=lambda i: i["slug"]):
        slug = icon["slug"]
        side = write_glyph(staging, slug, layers(src_dir, icon))
        colours = write_tile(staging, icon, src_dir, layers(src_dir, icon))
        rows.append(
            f"    ProgramIcon {{\n"
            f"        name: {rust_str(icon['name'])},\n"
            f"        slug: {rust_str(slug)},\n"
            f"        tint: 0x{tint(colours):06x},\n"
            f"        glyph: Icon {{ name: {rust_str('program-' + slug)}, view: {rust_f32(side)}, body: include_str!({rust_str('../../../resources/program-icons/glyphs/' + slug + '.svg')}) }},\n"
            f"        tile: Icon {{ name: {rust_str('program-tile-' + slug)}, view: {rust_f32(TILE_VIEW)}, body: include_str!({rust_str('../../../resources/program-icons/tiles/' + slug + '.svg')}) }},\n"
            f"    }},\n"
        )
    catalog = CATALOG.with_suffix(".rs.new")
    catalog.write_text(
        "//! Generated by `scripts/program-icons.py` from Superlogical's Rex icon set; do not edit.\n"
        "//! The artwork's terms are in `resources/program-icons/NOTICE.md`.\n\n"
        "use rune::Icon;\n\n"
        "use super::ProgramIcon;\n\n"
        "#[rustfmt::skip]\n"
        f"pub(super) static ALL: [ProgramIcon; {len(rows)}] = [\n" + "".join(rows) + "];\n"
    )
    for sub in ("tiles", "glyphs"):
        shutil.rmtree(OUT / sub, ignore_errors=True)
        (staging / sub).rename(OUT / sub)
    staging.rmdir()
    os.replace(catalog, CATALOG)
    print(f"{len(rows)} icons")


if __name__ == "__main__":
    main()
