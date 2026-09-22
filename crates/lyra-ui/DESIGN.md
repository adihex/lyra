# lyra-gui design contract

The measured visual contract for the GTK shell — one design language with
the macOS shell (`app/Sources/LyraApp/`). The single source of truth for
**colors, spacing, type sizes, radii, and stroke weights** is
`design/tokens.toml`; `make tokens` (`scripts/gen_design_tokens.py`)
stamps it into both shells — Swift `LyraColors`/`Bubble.*` and Rust
`Palette`/geometry consts. Do not introduce literal colors or per-pane
styling that bypasses the tokens.

## Scheme

The app follows the system: `prefs.appearance` (`system` default) maps to
`ColorScheme::PreferDark`, so a desktop dark-or-light preference resolves
to the matching palette row — and stock GTK widgets resolve the same
scheme — while `dark`/`light` pin `ForceDark`/`ForceLight`. Changing the
pref (Design Lab → appearance picker) or the system's own scheme flip
restamps the stylesheet live via `StyleManager::dark` notify →
`design::apply_theme`. `LYRA_SCHEME=light|dark` overrides the env for
headless verification. All six generated palette rows (lavender/rose/mint
× light/dark) are available at runtime: `design::palette_for(family, dark)`.

## Color tokens

`Palette` in `src/design.rs` mirrors `LyraColors` role-for-role; each role is
emitted as a `@define-color lyra_<role>` custom property. The generated
block holds all six rows; the active row is `palette_for(prefs.palette,
effective_dark())`. Stock-widget accent vars (`accent_color`,
`accent_bg_color`, `accent_fg_color`) are also stamped from the row so
switches, scale highlights, and dropdown checks follow the palette.

| Role | Token | Lavender dark | GTK usage |
|---|---|---|---|
| chassis | `@lyra_chassis` | `#211F29` | window + content background, list/scrolled surfaces |
| tray | `@lyra_tray` | `#19171F` | header bar, sidebar, transport + status bars, sunken text wells |
| keycap | `@lyra_keycap` | `#34303E` | raised surfaces: `.lyra-card`, keycap buttons, row hover |
| accent | `@lyra_accent` | `#BBA8CF` | selection wash (alpha 0.24) on track rows, gradient end |
| legend | `@lyra_legend` | `#ECE5F2` | primary text |
| onAccent | `@lyra_on_accent` | `#2B2336` | text on accent |
| tint | `@lyra_tint` | `#C5B3D8` | primary buttons, seek highlight, sidebar selection, focus ring |
| onTint | `@lyra_on_tint` | `#30263F` | text on tint |
| secondaryInk | `@lyra_secondary_ink` | `#A9C9C3` | `.mint` ink, EQ highlight, chips |
| companion | `@lyra_companion` | `#AFC9C3` | `.mint` ink on selected rows |
| border | `@lyra_border` | `#60566E` | hairlines, card borders, separators |
| dim | `@lyra_dim` | `#8E82A8` | derived de-emphasized text (not a LyraColors role) |

Rules must reference `@lyra_*` tokens only. A palette swap (rose/mint) is a
`Palette` value change, never a rule edit.

## Geometry / type tokens (generated)

`[space]`, `[type]`, `[radius]`, `[stroke]` in `design/tokens.toml` stamp
to `SPACE_*`, `TY_*`, `R_*`, `STROKE_*` consts (code layout) **and** into
`{NAME}` placeholders inside the CSS rule set — so stylesheet geometry is
contract-driven, not hand-duplicated.

| Token | Value | Where |
|---|---|---|
| `SPACE_*` | 4/8/12/16/20/24/page 28 | `pane_root()` margins (20v × 28h), spacing |
| `TY_*` | title 24 · headline 14 · caption 11 · micro 10 | `.lyra-title/headline/caption/micro` |
| `R_CARD` | 18 | `.lyra-card` radius (+1px rim, padding 14, **no shadow**) |
| `R_TRAY` / `R_FIELD` | 20 / 12 | tray curves; `entry`/`spinbutton` radius |
| `R_ROW` / `R_SIDEBAR_ROW` | 10 / 14 | track rows; sidebar pill rows |
| `STROKE_FINE` | 1 | hairlines, card/button rims |
| `STROKE_FOCUS` | 3 | `:focus-visible` rings, selection left-tab |
| Window | 1040×760 default, 780×560 min | matches `Bubble.Size.window*` |
| Buttons | pill (radius 999): `sharp` keycap+rim, `suggested-action` tint→accent gradient | `design::*_button()` |
| Transport keys | `lyra-key` 32px round keycap, `lyra-key-play` 40px tint gradient | Mac `compactKey`/`transportKey` |
| Seek trough | min-height 5, pill, tint→mint gradient | `scale.seek` |
| EQ trough | min-width 5, pill, mint highlight | `scale.eq` |
| Icons | symbolic (`*-symbolic`), 16px sidebar, tinted by ink role | GTK-native, always themed |

## States

- Track row hover → `keycap`; selected → 24% `accent` wash + 3px accent
  left tab, `legend` ink (`.dim` → 70% legend, `.mint` → `companion`).
- Sidebar selection → `tint`→`accent` 135° gradient pill, `onTint` ink
  (icon included); hover → 60% `keycap`.
- `:focus-visible` rings on buttons, sidebar rows, track rows, entries —
  3px `tint` ring (30–50% alpha); keyboard focus is never invisible.
- `suggested-action:disabled` → 45% opacity, flat `tint`.
- `entry:focus` → `tint` border + tint ring.
- Headerbar carries a faint `tint` sheen (10% → transparent).
- Design Lab pad selection → `.selected` on `button.lyra-pad` (tint→accent
  wash); simulated catalog states use `.sim-hover` / `.sim-pressed`
  classes since GTK can't force widget states — the Disabled column uses
  real `set_sensitive(false)`.
- Intentional differences from macOS: GTK list widgets instead of SwiftUI
  `Table`; symbolic icons in place of SF Symbols; emoji glyphs avoided
  (they tofu without a color-emoji font); depth is rim-contrast only —
  **no box-shadows**, matching the shadowless Bubblegum contract.

## Typography & text rules

- Type ramp is contract-fixed (`TY_*`); panes use `section_title()`,
  `dim_label()`, `.lyra-*` classes — no ad-hoc font sizes.
- Numbers that churn or align in columns get `design::mono()` — Pango
  `tnum`, the GTK `monospacedDigit()` equivalent (GTK CSS has no
  font-feature control, so it's a code attribute).
- Ellipsis `…` in placeholders and in-flight labels (`Filter…`,
  `Scanning…`), never `...`.
- Track cells ellipsize at end (`EllipsizeMode::End`); icon-only buttons
  carry tooltips (GTK's accessible-name surface).

## Shared primitives (reuse these, don't restyle)

`src/design.rs`: `pane_root()` (page margins), `card()` (raised surface),
`section_title()`, `dim_label()` (caption + dim), `mono()` (tnum),
`empty_state()` (symbolic glyph + headline + hint, centered),
`primary_button()` / `secondary_button()`. Panes compose these; add a
primitive when the same pattern appears twice, not inline CSS.

## Product truth per pane

- **Library / transport / EQ / coach / map**: real contract — `lyra_ffi`
  host state and IPC operations, identical to the macOS app.
- **Discover**: real contract — `lyra-search` handle results.
- **Remote**: real contract — pairing/device list via `lyra-remote`.
- **Visuals**: real contract — engine viz tap (`last_seq` deltas), never
  synthesized.
- **Design Lab**: honest by construction — a local-only mock deck + component
  catalog (no engine, no host); it labels itself a demo. It also carries
  the theme pickers (appearance + palette), mirroring the Mac's
  `BubbleDesignLabView`.
