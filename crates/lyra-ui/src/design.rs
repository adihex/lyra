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

// Geometry/type/radius/stroke contract — generated from design/tokens.toml
// ([space], [type], [radius], [stroke]); the same values feed Swift's
// Bubble.Space/TypeSize/Size/Stroke.
// GENERATED GEOMETRY — do not edit
#[allow(dead_code)] // full contract stamped;
pub const SPACE_XS: i32 = 4;
#[allow(dead_code)] // full contract stamped;
pub const SPACE_SM: i32 = 8;
#[allow(dead_code)] // full contract stamped;
pub const SPACE_MD: i32 = 12;
#[allow(dead_code)] // full contract stamped;
pub const SPACE_LG: i32 = 16;
#[allow(dead_code)] // full contract stamped;
pub const SPACE_XL: i32 = 20;
#[allow(dead_code)] // full contract stamped;
pub const SPACE_XXL: i32 = 24;
#[allow(dead_code)] // full contract stamped;
pub const SPACE_PAGE: i32 = 28;
#[allow(dead_code)] // full contract stamped;
pub const TY_TITLE: i32 = 24;
#[allow(dead_code)] // full contract stamped;
pub const TY_HEADLINE: i32 = 14;
#[allow(dead_code)] // full contract stamped;
pub const TY_BODY: i32 = 12;
#[allow(dead_code)] // full contract stamped;
pub const TY_CAPTION: i32 = 11;
#[allow(dead_code)] // full contract stamped;
pub const TY_MICRO: i32 = 10;
#[allow(dead_code)] // full contract stamped;
pub const TY_GLYPH: i32 = 20;
#[allow(dead_code)] // full contract stamped;
pub const R_CARD: i32 = 18;
#[allow(dead_code)] // full contract stamped;
pub const R_TRAY: i32 = 20;
#[allow(dead_code)] // full contract stamped;
pub const R_FIELD: i32 = 12;
#[allow(dead_code)] // full contract stamped;
pub const R_ROW: i32 = 10;
#[allow(dead_code)] // full contract stamped;
pub const R_SIDEBAR_ROW: i32 = 14;
#[allow(dead_code)] // full contract stamped;
pub const STROKE_FINE: i32 = 1;
#[allow(dead_code)] // full contract stamped;
pub const STROKE_CONTRAST: i32 = 2;
#[allow(dead_code)] // full contract stamped;
pub const STROKE_FOCUS: i32 = 3;
#[allow(dead_code)] // full contract stamped;
pub const STROKE_INSET: i32 = 3;
// END GENERATED GEOMETRY

/// Full stylesheet: token definitions first, then the rule set that maps
/// roles onto GTK's widget tree. Rules reference only `@lyra_*` colors —
/// never literal hex — so a palette swap is a `Palette` change only.
/// `{NAME}` placeholders are filled from the generated geometry consts, so
/// CSS geometry tracks the same contract as the code layout.
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
    let mut rules = RULES.to_string();
    for (key, value) in [
        ("R_CARD", R_CARD),
        ("R_TRAY", R_TRAY),
        ("R_FIELD", R_FIELD),
        ("R_ROW", R_ROW),
        ("R_SIDEBAR_ROW", R_SIDEBAR_ROW),
        ("TY_TITLE", TY_TITLE),
        ("TY_HEADLINE", TY_HEADLINE),
        ("TY_CAPTION", TY_CAPTION),
        ("TY_MICRO", TY_MICRO),
        ("TY_GLYPH", TY_GLYPH),
        ("STROKE_FINE", STROKE_FINE),
        ("STROKE_FOCUS", STROKE_FOCUS),
    ] {
        rules = rules.replace(&format!("{{{key}}}"), &value.to_string());
    }
    s.push_str(&rules);
    s
}

const RULES: &str = r#"
/* ═══ Surfaces — chassis (window) / tray (recessed) / keycap (raised),
   1:1 with the app's window/sidebar/card layering. Shadowless: depth
   comes from rim contrast, per the Bubblegum contract. */
window { background-color: @lyra_chassis; color: @lyra_legend; }
headerbar { background-color: @lyra_tray; color: @lyra_legend;
            background-image: linear-gradient(to bottom,
                alpha(@lyra_tint, 0.10), transparent 55%);
            box-shadow: 0 {STROKE_FINE}px 0 @lyra_border; }
headerbar .title { font-weight: 600; }
list, scrolledwindow, columnview, .view {
    background-color: @lyra_chassis; color: @lyra_legend; }
separator { background-color: alpha(@lyra_border, 0.5);
            min-height: {STROKE_FINE}px; min-width: {STROKE_FINE}px; }

/* ═══ Type ramp — matches Bubble.TypeSize roles 1:1. */
.lyra-title { font-size: {TY_TITLE}px; font-weight: 600; }
.lyra-headline { font-size: {TY_HEADLINE}px; font-weight: 600; }
.lyra-caption { font-size: {TY_CAPTION}px; }
.lyra-micro { font-size: {TY_MICRO}px; font-weight: 500; }
.dim { color: @lyra_dim; }
.mint { color: @lyra_secondary_ink; }

/* ═══ Sidebar — tray surface, icon+label pill rows, tint-filled pill on
   selection (keycap metaphor: the pressed key is the lit one). */
.lyra-sidebar { background-color: @lyra_tray; }
.lyra-sidebar row { padding: 8px 14px; border-radius: {R_SIDEBAR_ROW}px;
                    margin: 2px 8px; }
.lyra-sidebar row image { color: @lyra_dim; }
.lyra-sidebar row:hover { background-color: alpha(@lyra_keycap, 0.6); }
.lyra-sidebar row:selected {
    background-image: linear-gradient(135deg, @lyra_tint, @lyra_accent);
    color: @lyra_on_tint; }
.lyra-sidebar row:selected image { color: @lyra_on_tint; }
.lyra-sidebar row:focus-visible {
    box-shadow: 0 0 0 {STROKE_FOCUS}px alpha(@lyra_tint, 0.5); }

/* ═══ Transport tray + keycap transport. */
.lyra-transport { background-color: @lyra_tray;
                  border-top: {STROKE_FINE}px solid @lyra_border;
                  padding: 10px 16px; }
button.lyra-key { min-width: 32px; min-height: 32px; border-radius: 999px;
    padding: 0; background-color: @lyra_keycap;
    border: {STROKE_FINE}px solid alpha(@lyra_border, 0.8); color: @lyra_legend; }
button.lyra-key:hover { border-color: @lyra_tint; color: @lyra_tint; }
button.lyra-key-play { min-width: 40px; min-height: 40px; border-radius: 999px;
    padding: 0; border: none; color: @lyra_on_tint;
    background-image: linear-gradient(135deg, @lyra_tint, @lyra_accent); }
button.lyra-key-play:hover { box-shadow: 0 0 0 {STROKE_FOCUS}px alpha(@lyra_tint, 0.3); }

/* ═══ Card — the raised keycap surface: radius + rim, no shadow. */
.lyra-card { background-color: @lyra_keycap;
             border: {STROKE_FINE}px solid @lyra_border;
             border-radius: {R_CARD}px; padding: 14px; }

/* ═══ Buttons — pill caps; suggested-action is the lit key. */
button.sharp { border-radius: 999px; padding: 6px 16px;
    background-color: @lyra_keycap;
    border: {STROKE_FINE}px solid alpha(@lyra_border, 0.8); }
button.sharp:hover { border-color: @lyra_tint; }
button.flat { border-radius: 999px; }
button.suggested-action {
    background-image: linear-gradient(135deg, @lyra_tint, @lyra_accent);
    color: @lyra_on_tint;
    border-radius: 999px; padding: 6px 18px; border: none; }
button.suggested-action:hover {
    box-shadow: 0 0 0 {STROKE_FOCUS}px alpha(@lyra_tint, 0.3); }
button.suggested-action:disabled { opacity: 0.45; background-image: none;
    background-color: @lyra_tint; }
button:focus-visible {
    box-shadow: 0 0 0 {STROKE_FOCUS}px alpha(@lyra_tint, 0.5); }

/* ═══ Track table — header caps in micro type; rounded hover/selected rows;
   selection is an accent wash with a left tab. */
.track-header button { font-size: {TY_MICRO}px; font-weight: 600;
    color: @lyra_dim; background: none; border: none; padding: 6px 4px; }
.track-header button:hover { color: @lyra_legend; }
list.track > row { padding: 5px 10px; border-radius: {R_ROW}px; }
list.track > row:hover { background-color: @lyra_keycap; }
list.track > row:selected {
    background-color: alpha(@lyra_accent, 0.24);
    border-left: {STROKE_FOCUS}px solid @lyra_accent; }
list.track > row:selected .dim { color: alpha(@lyra_legend, 0.7); }
list.track > row:selected .mint { color: @lyra_companion; }
list.track > row:focus-visible {
    box-shadow: inset 0 0 0 {STROKE_FOCUS}px alpha(@lyra_tint, 0.5); }

/* ═══ Sliders — seek fades tint→mint; EQ is the secondary accent. */
scale.seek trough { min-height: 5px; border-radius: 999px; }
scale.seek highlight {
    background-image: linear-gradient(90deg, @lyra_tint, @lyra_secondary_ink);
    border-radius: 999px; }
scale.seek slider { border-radius: 999px; }
scale.eq trough { min-width: 5px; border-radius: 999px; }
scale.eq highlight { background-color: @lyra_secondary_ink;
                     border-radius: 999px; }

/* ═══ Sunken text wells — tray surface, field radius, tint focus ring. */
entry, spinbutton { background-color: @lyra_tray; color: @lyra_legend;
    caret-color: @lyra_legend;
    border: {STROKE_FINE}px solid @lyra_border;
    border-radius: {R_FIELD}px; padding: 6px 14px; }
entry:focus, spinbutton:focus { border-color: @lyra_tint;
    box-shadow: 0 0 0 {STROKE_FOCUS}px alpha(@lyra_tint, 0.3); }

/* ═══ Chips — mint wash + rim, the app's badge treatment. */
.lyra-chip { background-color: alpha(@lyra_secondary_ink, 0.14);
    border: {STROKE_FINE}px solid alpha(@lyra_secondary_ink, 0.4);
    border-radius: 999px; padding: 3px 10px; }

/* ═══ Status bar — recessed tray, micro caption. */
.lyra-statusbar { background-color: @lyra_tray;
    border-top: {STROKE_FINE}px solid @lyra_border;
    padding: 4px 16px; }

"#;

// ── Shared primitives ──────────────────────────────────────────────────────
// The widget vocabulary panes build from. Each maps to a role in DESIGN.md.

/// Vertical pane container — page margins on the shared grid
/// (SPACE_XL vertical, SPACE_PAGE horizontal, SPACE_MD spacing).
pub fn pane_root() -> gtk4::Box {
    let b = gtk4::Box::new(gtk4::Orientation::Vertical, SPACE_MD);
    b.set_margin_top(SPACE_XL);
    b.set_margin_bottom(SPACE_XL);
    b.set_margin_start(SPACE_PAGE);
    b.set_margin_end(SPACE_PAGE);
    b
}

/// Raised keycap surface — card styling on a box of either orientation.
pub fn card(orientation: gtk4::Orientation) -> gtk4::Box {
    let b = gtk4::Box::new(orientation, SPACE_SM);
    b.add_css_class("lyra-card");
    b
}

/// Pane heading — the `title` role of the shared ramp.
pub fn section_title(text: &str) -> gtk4::Label {
    let l = gtk4::Label::new(Some(text));
    l.add_css_class("lyra-title");
    l.set_xalign(0.0);
    l
}

/// De-emphasized text — status lines, secondary columns, hints.
pub fn dim_label(text: &str) -> gtk4::Label {
    let l = gtk4::Label::new(Some(text));
    l.add_css_class("dim");
    l.add_css_class("lyra-caption");
    l
}

/// Tabular digits — Pango `tnum` on a label, the GTK equivalent of
/// SwiftUI's `.monospacedDigit()`. GTK CSS carries no font-feature
/// control, so this is a code-level attribute.
pub fn mono(label: &gtk4::Label) {
    let attrs = gtk4::pango::AttrList::new();
    attrs.insert(gtk4::pango::AttrFontFeatures::new("tnum"));
    label.set_attributes(Some(&attrs));
}

/// Centered empty-state block — large symbolic glyph + headline + dim
/// hint, matching the macOS shell's "No tunes yet" treatment.
pub fn empty_state(icon: &str, headline: &str, hint: &str) -> gtk4::Box {
    let b = gtk4::Box::new(gtk4::Orientation::Vertical, SPACE_SM);
    b.set_valign(gtk4::Align::Center);
    b.set_vexpand(true);
    let g = gtk4::Image::from_icon_name(icon);
    g.set_pixel_size(44);
    g.add_css_class("dim");
    b.append(&g);
    let h = gtk4::Label::new(Some(headline));
    h.add_css_class("lyra-headline");
    b.append(&h);
    let d = dim_label(hint);
    d.set_wrap(true);
    d.set_justify(gtk4::Justification::Center);
    b.append(&d);
    b
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
