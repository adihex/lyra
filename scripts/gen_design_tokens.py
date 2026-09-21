#!/usr/bin/env python3
"""Regenerate design-token tables in both shells from design/tokens.toml.

Stamps the BEGIN/END GENERATED blocks in:
  - app/Sources/LyraApp/LyraTheme.swift   (Swift `LyraColors` switch cases)
  - crates/lyra-ui/src/design.rs         (Rust `Palette` consts)

Zero dependencies — parses the flat [palette.scheme] tables with a small
regex reader so it runs on any Python 3 (no tomllib requirement).

Usage: python3 scripts/gen_design_tokens.py [--check]
  --check   exit non-zero if a generated file would change (CI guard)
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOKENS = ROOT / "design" / "tokens.toml"
SWIFT = ROOT / "app" / "Sources" / "LyraApp" / "LyraTheme.swift"
RUST = ROOT / "crates" / "lyra-ui" / "src" / "design.rs"

BEGIN, END = "// GENERATED PALETTES — do not edit", "// END GENERATED PALETTES"

SECTION = re.compile(r"^\s*\[([\w.]+)\]\s*$")
KV = re.compile(r'^\s*(\w+)\s*=\s*"?(#[0-9A-Fa-f]{6})"?\s*$')
CAMEL = {
    "on_accent": "onAccent",
    "on_tint": "onTint",
    "secondary_ink": "secondaryInk",
}
SWIFT_ROLES = [
    "chassis", "tray", "keycap", "accent", "legend", "on_accent",
    "tint", "on_tint", "secondary_ink", "companion", "border",
]
PALETTES = ["lavender", "rose", "mint"]


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
        if m and cur and not cur.startswith(("meta", "roles")):
            tables[cur][m.group(1)] = m.group(2).lstrip("#").upper()
    return tables


def swift_block(p):
    lines = []
    for name in PALETTES:
        for dark in (False, True):
            t = p[f"{name}.{'dark' if dark else 'light'}"]
            roles = ", ".join(
                f"{CAMEL.get(r, r)}: 0x{t[r]}" for r in SWIFT_ROLES)
            lines.append(
                f"        case (.{name}, {str(dark).lower()}):\n"
                f"            return LyraColors({roles})")
    return "\n".join(lines) + "\n"


def rust_block(p, default):
    out = []
    for name in PALETTES:
        for scheme in ("light", "dark"):
            t = p[f"{name}.{scheme}"]
            fields = "".join(
                f"    {r}: \"#{t[r]}\",\n" for r in t
            )
            out.append(
                f"#[allow(dead_code)] // every row is stamped for parity;\n"
                f"pub const {name.upper()}_{scheme.upper()}: Palette ="
                f" Palette {{\n{fields}}};\n")
    pname, pscheme = default.split(".")
    out.append(f"/// Default GTK palette — `{default}` per design/tokens.toml.\n"
               f"pub const PALETTE: Palette = {pname.upper()}_{pscheme.upper()};")
    return "\n".join(out)


def stamp(path, block):
    text = path.read_text()
    start = text.index(BEGIN) + len(BEGIN)
    end = text.index(END)
    # keep the END marker's own indentation (Swift: inside switch; Rust: col 0)
    indent = re.search(r"[ \t]*$", text[:end]).group(0)
    return text[:start] + "\n" + block.rstrip("\n") + "\n" + indent + text[end:]


def main():
    p = parse_tokens(TOKENS)
    default = "lavender.dark"
    for line in TOKENS.read_text().splitlines():
        m = re.match(r'\s*gtk_default\s*=\s*"([\w.]+)"', line)
        if m:
            default = m.group(1)
    outs = {SWIFT: stamp(SWIFT, swift_block(p)),
            RUST: stamp(RUST, rust_block(p, default))}
    if "--check" in sys.argv:
        dirty = [str(f) for f, s in outs.items() if s != f.read_text()]
        if dirty:
            print("stale generated blocks:", *dirty, sep="\n  ")
            sys.exit(1)
        return
    for f, s in outs.items():
        f.write_text(s)
        print("stamped", f.relative_to(ROOT))


if __name__ == "__main__":
    main()
