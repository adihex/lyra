# lyra-gui design contract

The measured visual contract for the GTK shell. Its source of truth is the
macOS app: `app/Sources/LyraApp/LyraTheme.swift` (`LyraColors` role names +
`lavender` dark row hex values). `src/design.rs` projects that contract onto
GTK — do not introduce literal colors or per-pane styling that bypasses it.

## Scheme

The app is dark-only on Linux for now: `AdwStyleManager` is pinned to
`ColorScheme::ForceDark` at startup so stock GTK widgets (header bar, lists,
entries, dialogs) resolve to dark adwaita colors even when the desktop
reports no dark preference. All palette tokens below assume dark.

## Color tokens

`Palette` in `src/design.rs` mirrors `LyraColors` role-for-role; each role is
emitted as a `@define-color lyra_<role>` custom property.

| Role | Token | Lavender dark | GTK usage |
|---|---|---|---|
| chassis | `@lyra_chassis` | `#211F29` | window + content background, list/scrolled surfaces |
| tray | `@lyra_tray` | `#19171F` | header bar, sidebar, transport bar, sunken text wells |
| keycap | `@lyra_keycap` | `#34303E` | raised surfaces: `.lyra-card`, row hover |
| accent | `@lyra_accent` | `#BBA8CF` | selection wash (alpha 0.22) on track rows |
| legend | `@lyra_legend` | `#ECE5F2` | primary text |
| onAccent | `@lyra_on_accent` | `#2B2336` | text on accent |
| tint | `@lyra_tint` | `#C5B3D8` | primary buttons, seek highlight, sidebar selection, focus ring |
| onTint | `@lyra_on_tint` | `#30263F` | text on tint |
| secondaryInk | `@lyra_secondary_ink` | `#A9C9C3` | `.mint` ink, EQ highlight |
| companion | `@lyra_companion` | `#AFC9C3` | `.mint` ink on selected rows |
| border | `@lyra_border` | `#60566E` | hairlines, card borders, separators |
| dim | `@lyra_dim` | `#8E82A8` | derived de-emphasized text (not a LyraColors role) |

Rules must reference `@lyra_*` tokens only. A palette swap (rose/mint) is a
`Palette` value change, never a rule edit.

## Geometry

| Token | Value | Where |
|---|---|---|
| Pane margins | 16 top/bottom, 20 start/end | `design::pane_root()` |
| Card | radius 18, padding 14, 1px border, soft shadow | `design::card()` / `.lyra-card` |
| Sharp button | pill (radius 999), padding 6×16 | `design::secondary_button()` |
| Suggested action | pill, tint→accent gradient, hover lift | `design::primary_button()` |
| Sidebar row | radius 14, padding 8×14, weight 500 | `.lyra-sidebar row` |
| Track row | radius 10, padding 5×10 | `list.track > row` |
| Track header cell | font 11px, weight 600, `dim` ink | `.track-header button` |
| Entry / spinbutton | radius 14, padding 6×14 | `entry`, `spinbutton` |
| Seek trough | min-height 5, pill, tint→mint gradient fill | `scale.seek` |
| EQ trough | min-width 5, pill, mint highlight | `scale.eq` |

## States

- Track row hover → `keycap`; selected → 24% `accent` wash with a 3px
  accent left tab, `legend` ink (`.dim` → 70% legend, `.mint` →
  `companion` inside selection).
- Sidebar selection → `tint`→`accent` 135° gradient pill with `onTint`
  ink and a soft tint shadow; hover → 60% `keycap`.
- `suggested-action:disabled` → 45% opacity, flat `tint` (no gradient).
- `entry:focus` → `tint` border + 2px tint ring (30% alpha).
- Headerbar carries a faint `tint` sheen (10% → transparent).
- Intentional differences from macOS: GTK list/table widgets instead of
  SwiftUI `Table`; no hover-reveal affordances; the Linux shell leans
  rounder/more playful (pill buttons, gradients, left-tab selection) —
  whimsy lives in the GTK layer, the token contract stays identical.

## Shared primitives (reuse these, don't restyle)

`src/design.rs`: `pane_root()` (pane container), `card()` (raised surface),
`section_title()` (`title-1`), `dim_label()` (secondary ink),
`primary_button()` / `secondary_button()` (action roles). Panes compose
these; add a primitive when the same pattern appears twice, not inline CSS.

## Product truth per pane

- **Library / transport / EQ / coach / map**: real contract — `lyra_ffi`
  host state and IPC operations, identical to the macOS app.
- **Discover**: real contract — `lyra-search` handle results.
- **Remote**: real contract — pairing/device list via `lyra-remote`.
- **Visuals**: real contract — engine viz tap (`last_seq` deltas), never
  synthesized.
