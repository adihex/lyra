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

// Palette values are generated from `design/tokens.toml` — edit there,
// then `make tokens` to restamp this block and the Swift `LyraColors` table.
// GENERATED PALETTES — do not edit
#[allow(dead_code)] // every row is stamped for parity;
pub const LAVENDER_LIGHT: Palette = Palette {
    chassis: "#F3F0F7",
    tray: "#E0D9E9",
    keycap: "#FDFCFF",
    accent: "#C5B3D8",
    legend: "#4A3C60",
    on_accent: "#30263F",
    tint: "#665379",
    on_tint: "#FFFFFF",
    secondary_ink: "#495F62",
    companion: "#C0D7D0",
    border: "#B8AAC7",
    dim: "#665379",
};

#[allow(dead_code)] // every row is stamped for parity;
pub const LAVENDER_DARK: Palette = Palette {
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

#[allow(dead_code)] // every row is stamped for parity;
pub const ROSE_LIGHT: Palette = Palette {
    chassis: "#F8F1F2",
    tray: "#EADCE1",
    keycap: "#FFFCFD",
    accent: "#D8B7C1",
    legend: "#60434E",
    on_accent: "#3D2932",
    tint: "#805468",
    on_tint: "#FFFFFF",
    secondary_ink: "#56614A",
    companion: "#CDD8BD",
    border: "#C6AAB5",
    dim: "#805468",
};

#[allow(dead_code)] // every row is stamped for parity;
pub const ROSE_DARK: Palette = Palette {
    chassis: "#281F23",
    tray: "#20181C",
    keycap: "#3C2F35",
    accent: "#D3AFBD",
    legend: "#F5E5EC",
    on_accent: "#39232D",
    tint: "#DDBFCC",
    on_tint: "#39232D",
    secondary_ink: "#C2CDAF",
    companion: "#C6D1B7",
    border: "#735766",
    dim: "#A99DA8",
};

#[allow(dead_code)] // every row is stamped for parity;
pub const MINT_LIGHT: Palette = Palette {
    chassis: "#EFF5F2",
    tray: "#D7E4DD",
    keycap: "#FCFFFD",
    accent: "#AECABD",
    legend: "#354F43",
    on_accent: "#24382E",
    tint: "#496B5A",
    on_tint: "#FFFFFF",
    secondary_ink: "#5D5A77",
    companion: "#CECBE0",
    border: "#A6BDB0",
    dim: "#496B5A",
};

#[allow(dead_code)] // every row is stamped for parity;
pub const MINT_DARK: Palette = Palette {
    chassis: "#1C2522",
    tray: "#141C19",
    keycap: "#2C3C34",
    accent: "#A3C5B4",
    legend: "#E1F0E8",
    on_accent: "#1F362A",
    tint: "#B5D5C4",
    on_tint: "#24382E",
    secondary_ink: "#C8BFDC",
    companion: "#C6BED7",
    border: "#4D6B5D",
    dim: "#8FA39B",
};

/// Default GTK palette — `lavender.dark` per design/tokens.toml.
pub const PALETTE: Palette = LAVENDER_DARK;
// END GENERATED PALETTES

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
            background-image: linear-gradient(to bottom,
                alpha(@lyra_tint, 0.10), transparent 55%);
            box-shadow: 0 1px 0 @lyra_border; }
list, scrolledwindow, columnview, .view {
    background-color: @lyra_chassis; color: @lyra_legend; }
separator { background-color: alpha(@lyra_border, 0.5); min-height: 1px; }

/* Sidebar — tray surface, pill rows, gradient tint-filled selection. */
.lyra-sidebar { background-color: @lyra_tray; }
.lyra-sidebar row { padding: 8px 14px; border-radius: 14px;
                    font-weight: 500; }
.lyra-sidebar row:hover { background-color: alpha(@lyra_keycap, 0.6); }
.lyra-sidebar row:selected {
    background-image: linear-gradient(135deg, @lyra_tint, @lyra_accent);
    color: @lyra_on_tint;
    box-shadow: 0 2px 8px alpha(@lyra_tint, 0.45); }

/* Transport tray. */
.lyra-now-playing { background-color: @lyra_tray;
                    border-top: 1px solid @lyra_border; padding: 10px 16px; }

/* Card — the raised keycap surface: soft radius, ambient shadow. */
.lyra-card { background-color: @lyra_keycap;
             border: 1px solid @lyra_border;
             border-radius: 18px; padding: 14px;
             box-shadow: 0 2px 10px alpha(@lyra_border, 0.30); }

/* Buttons — pill caps; suggested-action carries the tint→accent gradient
   and lifts on hover. */
button.sharp { border-radius: 999px; padding: 6px 16px; }
button.flat { border-radius: 999px; }
button.suggested-action {
    background-image: linear-gradient(135deg, @lyra_tint, @lyra_accent);
    color: @lyra_on_tint;
    border-radius: 999px; padding: 6px 18px; border: none; }
button.suggested-action:hover {
    box-shadow: 0 3px 10px alpha(@lyra_tint, 0.5); }
button.suggested-action:disabled { opacity: 0.45; background-image: none;
    background-color: @lyra_tint; }

/* Track table — `track` is on the ListBox, so rows are `list.track > row`.
   Rounded rows; selection is an accent-wash pill with a playful left tab. */
.track-header button { font-weight: 600; font-size: 11px; color: @lyra_dim;
                       background: none; border: none; padding: 6px 4px; }
.track-header button:hover { color: @lyra_legend; }
list.track > row { padding: 5px 10px; border-radius: 10px; }
list.track > row:hover { background-color: @lyra_keycap; }
list.track > row:selected {
    background-color: alpha(@lyra_accent, 0.24);
    border-left: 3px solid @lyra_accent; }
list.track > row:selected .dim { color: alpha(@lyra_legend, 0.7); }
list.track > row:selected .mint { color: @lyra_companion; }

/* Sliders — seek fades tint→mint along its length; EQ is the secondary. */
scale.seek trough { min-height: 5px; border-radius: 999px; }
scale.seek highlight {
    background-image: linear-gradient(90deg, @lyra_tint, @lyra_secondary_ink);
    border-radius: 999px; }
scale.seek slider { border-radius: 999px; }
scale.eq trough { min-width: 5px; border-radius: 999px; }
scale.eq highlight { background-color: @lyra_secondary_ink;
                     border-radius: 999px; }

/* Sunken text wells take the tray surface; soft pill radius, tint focus. */
entry, spinbutton { background-color: @lyra_tray; color: @lyra_legend;
                    caret-color: @lyra_legend;
                    border: 1px solid @lyra_border;
                    border-radius: 14px; padding: 6px 14px; }
entry:focus, spinbutton:focus { border-color: @lyra_tint;
    box-shadow: 0 0 0 2px alpha(@lyra_tint, 0.3); }

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
