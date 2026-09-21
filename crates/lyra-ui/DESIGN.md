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
| Card | radius 12, padding 12, 1px border | `design::card()` / `.lyra-card` |
| Sharp button | radius 8, padding 5×12 | `design::secondary_button()` |
| Suggested action | radius 8, padding 5×14 | `design::primary_button()` |
| Sidebar row | radius 10, padding 8×12, weight 500 | `.lyra-sidebar row` |
| Track row | padding 4×8 | `list.track > row` |
| Track header cell | font 11px, weight 600, `dim` ink | `.track-header button` |
| Seek trough | min-height 4, tint highlight | `scale.seek` |
| EQ trough | min-width 4, mint highlight | `scale.eq` |

## States

- Track row hover → `keycap`; selected → 22% `accent` wash, `legend` ink
  (`.dim` → 70% legend, `.mint` → `companion` inside selection).
- Sidebar selection → solid `tint` with `onTint` ink.
- `suggested-action:disabled` → 45% opacity.
- `entry:focus` → `tint` border.
- Intentional differences from macOS: GTK list/table widgets instead of
  SwiftUI `Table`; no hover-reveal affordances; disabled-state and focus
  treatments follow GTK/adwaita norms.

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
