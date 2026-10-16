#!/usr/bin/env python3
"""Regenerate design-token tables in both shells from design/tokens.toml.

Stamps the marked GENERATED blocks in:
  - app/Sources/LyraApp/LyraTheme.swift   (Swift LyraColors cases + Bubble tokens)
  - crates/lyra-ui/src/design.rs         (Rust Palette consts + geometry consts)

Zero dependencies — parses the flat TOML tables with a small regex reader
so it runs on any Python 3 (no tomllib requirement).

Usage: python3 scripts/gen_design_tokens.py [--check]
  --check   exit non-zero if a generated file would change (CI guard)
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOKENS = ROOT / "design" / "tokens.toml"
SWIFT = ROOT / "app" / "Sources" / "LyraApp" / "LyraTheme.swift"
SWIFT_TOKENS = ROOT / "app" / "Sources" / "LyraApp" / "Theme.swift"
RUST = ROOT / "crates" / "lyra-ui" / "src" / "design.rs"

SECTION = re.compile(r"^\s*\[([\w.]+)\]\s*$")
KV = re.compile(r'^\s*(\w+)\s*=\s*"?([^\s"]+)"?\s*$')
DEFAULT_RE = re.compile(r'\s*gtk_default\s*=\s*"([\w.]+)"')
CAMEL = {
    "on_accent": "onAccent",
    "on_tint": "onTint",
    "secondary_ink": "secondaryInk",
    "sidebar_row": "sidebarRow",
}
SWIFT_ROLES = [
    "chassis", "tray", "keycap", "accent", "legend", "on_accent",
    "tint", "on_tint", "secondary_ink", "companion", "border",
]
PALETTES = ["lavender", "rose", "mint"]
NUM_SECTIONS = ["space", "type", "radius", "stroke"]


def parse_tokens(path):
    tables, cur = {}, None
    for line in path.read_text().splitlines():
        if line.lstrip().startswith("#"):
            continue
        m = SECTION.match(line)
        if m:
            cur = m.group(1)
            tables[cur] = {}
            continue
        m = KV.match(line)
        if m and cur:
            tables[cur][m.group(1)] = m.group(2)
    return tables


def ints(table):
    return {k: int(v) for k, v in table.items() if v.isdigit()}


# ---------- Swift emitters ----------

def swift_palettes(t):
    lines = []
    for name in PALETTES:
        for dark in (False, True):
            tb = t[f"{name}.{'dark' if dark else 'light'}"]
            roles = ", ".join(
                f"{CAMEL.get(r, r)}: 0x{tb[r].lstrip('#').upper()}"
                for r in SWIFT_ROLES)
            lines.append(
                f"        case (.{name}, {str(dark).lower()}):\n"
                f"            return LyraColors({roles})")
    return "\n".join(lines) + "\n"


def swift_nums(t, section, indent, prefix=""):
    out = []
    for k, v in ints(t[section]).items():
        name = f"{CAMEL.get(k, k)}{'Radius' if section == 'radius' else ''}"
        out.append(f"{indent}static let {prefix}{name}: CGFloat = {v}")
    return "\n".join(out) + "\n"


# ---------- Rust emitters ----------

def rust_palettes(t, default):
    out = []
    for name in PALETTES:
        for scheme in ("light", "dark"):
            tb = t[f"{name}.{scheme}"]
            fields = "".join(
                f'    {r}: "#{tb[r].lstrip("#").upper()}",\n' for r in tb
            )
            out.append(
                f"#[allow(dead_code)] // every row is stamped for parity;\n"
                f"pub const {name.upper()}_{scheme.upper()}: Palette ="
                f" Palette {{\n{fields}}};\n")
    pname, pscheme = default.split(".")
    out.append(
        f"/// Default GTK palette — `{default}` per design/tokens.toml.\n"
        f"#[allow(dead_code)] // reference row; palette_for() drives runtime.\n"
        f"pub const PALETTE: Palette = {pname.upper()}_{pscheme.upper()};")
    return "\n".join(out) + "\n"


def rust_geometry(t):
    out = []
    for section in NUM_SECTIONS:
        pfx = {"space": "SPACE", "type": "TY", "radius": "R",
               "stroke": "STROKE"}[section]
        for k, v in ints(t[section]).items():
            out.append(f"#[allow(dead_code)] // full contract stamped;\n"
                       f"pub const {pfx}_{k.upper()}: i32 = {v};")
    return "\n".join(out) + "\n"


# ---------- stamping ----------

def stamp(text, marker, block):
    begin, end = f"// GENERATED {marker} — do not edit", f"// END GENERATED {marker}"
    start = text.index(begin) + len(begin)
    stop = text.index(end)
    indent = re.search(r"[ \t]*$", text[:stop]).group(0)
    return text[:start] + "\n" + block.rstrip("\n") + "\n" + indent + text[stop:]


def main():
    t = parse_tokens(TOKENS)
    default = "lavender.dark"
    for line in TOKENS.read_text().splitlines():
        m = DEFAULT_RE.match(line)
        if m:
            default = m.group(1)
    plan = {
        SWIFT: [
            ("PALETTES", swift_palettes(t)),
        ],
        SWIFT_TOKENS: [
            ("SPACE", swift_nums(t, "space", "        ")),
            ("TYPE", swift_nums(t, "type", "        ")),
            ("SIZE", swift_nums(t, "radius", "        ")),
            ("STROKE", swift_nums(t, "stroke", "        ")),
        ],
        RUST: [
            ("PALETTES", rust_palettes(t, default)),
            ("GEOMETRY", rust_geometry(t)),
        ],
    }
    outs = {}
    for path, blocks in plan.items():
        text = path.read_text()
        for marker, block in blocks:
            text = stamp(text, marker, block)
        outs[path] = text
    if "--check" in sys.argv:
        dirty = [str(f) for f, s in outs.items() if s != f.read_text()]
        if dirty:
            print("stale generated blocks:", *dirty, sep="\n  ")
            sys.exit(1)
        return
    for f, s in outs.items():
        if s != f.read_text():
            f.write_text(s)
            print("stamped", f.relative_to(ROOT))


if __name__ == "__main__":
    main()
