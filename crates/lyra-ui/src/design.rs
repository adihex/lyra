//! Design system for `lyra-gui` — the shared visual contract with the macOS
//! SwiftUI shell (`app/Sources/LyraApp/LyraTheme.swift`).
//!
//! **Tokens.** `Palette` mirrors `LyraColors` role-for-role (chassis, tray,
//! keycap, accent, legend, onAccent, tint, onTint, secondaryInk, companion,
//! border); hex values are the same contract, not approximations. GTK gets
//! them as `@define-color lyra_*` custom properties so every rule below and
//! any future pane derives from the same source of truth.
//!
//! **Primitives.** Widget constructors (`pane_root`, `card`, `section_title`,
//! `dim_label`, `primary_button`, `secondary_button`) are the reusable
//! building blocks panes compose from — the GTK equivalent of the app's
//! shared SwiftUI modifiers. Panes should use these instead of re-specifying
//! margins, classes, or button roles inline.
//!
//! The full measured contract (geometry, states, typography) lives in
//! `crates/lyra-ui/DESIGN.md`.

use gtk4::prelude::*;

/// One palette = one `LyraColors` row. `PALETTE` is the default scheme:
/// lavender, dark — matching `Palette.lavender.colors(dark: true)`.
pub struct Palette {
    /// App window / content background.
    pub chassis: &'static str,
    /// Sidebar, header bar, now-playing tray, sunken wells (entries).
    pub tray: &'static str,
    /// Raised surfaces: cards, hovered rows.
    pub keycap: &'static str,
    /// Selection fill on lists.
    pub accent: &'static str,
    /// Primary text.
    pub legend: &'static str,
    /// Text on `accent`.
    pub on_accent: &'static str,
    /// Primary action fill (buttons, seek highlight).
    pub tint: &'static str,
    /// Text on `tint`.
    pub on_tint: &'static str,
    /// Secondary accent (mint) — badges, EQ highlight.
    pub secondary_ink: &'static str,
    /// Companion secondary — badges on selected rows.
    pub companion: &'static str,
    /// Hairlines and card borders.
    pub border: &'static str,
    /// Derived de-emphasized text (between legend and border).
    pub dim: &'static str,
}

pub const PALETTE: Palette = Palette {
    chassis: "#211F29",
    tray: "#19171F",
    keycap: "#34303E",
    accent: "#BBA8CF",
    legend: "#ECE5F2",
    on_accent: "#2B2336",
    tint: "#C5B3D8",
    on_tint: "#30263F",
    secondary_ink: "#A9C9C3",
    companion: "#AFC9C3",
    border: "#60566E",
    dim: "#8E82A8",
};

/// Full stylesheet: token definitions first, then the rule set that maps
/// roles onto GTK's widget tree. Rules reference only `@lyra_*` colors —
/// never literal hex — so a palette swap is a `Palette` change only.
pub fn css() -> String {
    let p = &PALETTE;
    let mut s = String::new();
    for (name, value) in [
        ("chassis", p.chassis),
        ("tray", p.tray),
        ("keycap", p.keycap),
        ("accent", p.accent),
        ("legend", p.legend),
        ("on_accent", p.on_accent),
        ("tint", p.tint),
        ("on_tint", p.on_tint),
        ("secondary_ink", p.secondary_ink),
        ("companion", p.companion),
        ("border", p.border),
        ("dim", p.dim),
    ] {
        s.push_str(&format!("@define-color lyra_{name} {value};\n"));
    }
    s.push_str(RULES);
    s
}

const RULES: &str = r#"
/* Surfaces — the window chassis, sunken tray, and raised keycap map 1:1 to
   the app's window/sidebar/card layering. Without these, GTK's scheme
   colors leak through on list and scrolled views. */
window { background-color: @lyra_chassis; color: @lyra_legend; }
headerbar { background-color: @lyra_tray; color: @lyra_legend;
            box-shadow: 0 1px 0 @lyra_border; }
list, scrolledwindow, columnview, .view {
    background-color: @lyra_chassis; color: @lyra_legend; }
separator { background-color: alpha(@lyra_border, 0.5); min-height: 1px; }

/* Sidebar — tray surface, pill rows, tint-filled selection. */
.lyra-sidebar { background-color: @lyra_tray; }
.lyra-sidebar row { padding: 8px 12px; border-radius: 10px;
                    font-weight: 500; }
.lyra-sidebar row:selected { background-color: @lyra_tint;
                             color: @lyra_on_tint; }

/* Transport tray. */
.lyra-now-playing { background-color: @lyra_tray;
                    border-top: 1px solid @lyra_border; padding: 10px 16px; }

/* Card — the raised keycap surface used for wells, chips, dialogs. */
.lyra-card { background-color: @lyra_keycap;
             border: 1px solid @lyra_border;
             border-radius: 12px; padding: 12px; }

/* Buttons — "sharp" is the neutral chrome, suggested-action the tint. */
button.sharp { border-radius: 8px; padding: 5px 12px; }
button.flat { border-radius: 8px; }
button.suggested-action { background-color: @lyra_tint; color: @lyra_on_tint;
                          border-radius: 8px; padding: 5px 14px; }
button.suggested-action:disabled { opacity: 0.45; }

/* Track table — `track` is on the ListBox, so rows are `list.track > row`.
   Hover lifts to keycap; selection uses the accent wash with legend ink. */
.track-header button { font-weight: 600; font-size: 11px; color: @lyra_dim;
                       background: none; border: none; padding: 6px 4px; }
.track-header button:hover { color: @lyra_legend; }
list.track > row { padding: 4px 8px; }
list.track > row:hover { background-color: @lyra_keycap; }
list.track > row:selected { background-color: alpha(@lyra_accent, 0.22); }
list.track > row:selected .dim { color: alpha(@lyra_legend, 0.7); }
list.track > row:selected .mint { color: @lyra_companion; }

/* Sliders — seek is tint, EQ is the mint secondary. */
scale.seek trough { min-height: 4px; }
scale.seek highlight { background-color: @lyra_tint; }
scale.eq trough { min-width: 4px; }
scale.eq highlight { background-color: @lyra_secondary_ink; }

/* Sunken text wells take the tray surface. */
entry, spinbutton { background-color: @lyra_tray; color: @lyra_legend;
                    caret-color: @lyra_legend;
                    border: 1px solid @lyra_border;
                    border-radius: 8px; padding: 5px 10px; }
entry:focus, spinbutton:focus { border-color: @lyra_tint; }

/* Ink roles. */
.dim { color: @lyra_dim; }
.mint { color: @lyra_secondary_ink; }
"#;

// ── Shared primitives ──────────────────────────────────────────────────────
// The widget vocabulary panes build from. Each maps to a role in DESIGN.md.

/// Vertical pane container with the standard content margins
/// (16 top/bottom, 20 left/right, 12 spacing).
pub fn pane_root() -> gtk4::Box {
    let b = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    b.set_margin_top(16);
    b.set_margin_bottom(16);
    b.set_margin_start(20);
    b.set_margin_end(20);
    b
}

/// Raised keycap surface — card styling on a box of either orientation.
pub fn card(orientation: gtk4::Orientation) -> gtk4::Box {
    let b = gtk4::Box::new(orientation, 8);
    b.add_css_class("lyra-card");
    b
}

/// Pane heading — the `title-1` role.
pub fn section_title(text: &str) -> gtk4::Label {
    let l = gtk4::Label::new(Some(text));
    l.add_css_class("title-1");
    l.set_xalign(0.0);
    l
}

/// De-emphasized text — status lines, secondary columns, hints.
pub fn dim_label(text: &str) -> gtk4::Label {
    let l = gtk4::Label::new(Some(text));
    l.add_css_class("dim");
    l
}

/// The pane's primary action — tint-filled suggested-action button.
pub fn primary_button(label: &str) -> gtk4::Button {
    let b = gtk4::Button::with_label(label);
    b.add_css_class("suggested-action");
    b
}

/// Neutral chrome button for secondary actions.
pub fn secondary_button(label: &str) -> gtk4::Button {
    let b = gtk4::Button::with_label(label);
    b.add_css_class("sharp");
    b
}
