//! Design Lab — the GTK port of the macOS `BubbleDesignLabView`.
//!
//! Two surfaces behind a segmented switcher:
//!   * Layout Canvas — an interactive 3×3 mock key matrix plus an
//!     inspector tray (name, density, legends, move, level, reset).
//!     Purely local state: no engine, no host, no audio.
//!   * Components — a catalog rendering each shared control in
//!     Normal / Hover / Pressed / Disabled. GTK can't force widget
//!     states, so cells stamp `.sim-hover` / `.sim-pressed` classes —
//!     see RULES — and real disabled controls in the last column.
//!     A live-controls row underneath exercises real events.
//!
//! The header row also carries the lab's theme pickers (appearance +
//! palette), matching the Mac's: appearance drives AdwStyleManager,
//! palette restamps the generated tokens live via design::apply_theme.

use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::{design, Shared};

// ── canvas state ─────────────────────────────────────────────────────────

/// One pad in the demo matrix — title + a bundled symbolic icon (the
/// Mac's SF Symbols don't exist on Linux; these are the Adwaita kin).
const PADS: [(&str, &str); 9] = [
    ("Kick", "media-record-symbolic"),
    ("Snare", "audio-speakers-symbolic"),
    ("Hat", "weather-clear-symbolic"),
    ("Clap", "face-smile-symbolic"),
    ("Bass", "audio-volume-high-symbolic"),
    ("Keys", "input-keyboard-symbolic"),
    ("Lead", "weather-storm-symbolic"),
    ("Chord", "emblem-music-symbolic"),
    ("FX", "starred-symbolic"),
];

/// Key spacing for the matrix — exercises the space contract, not the key size.
const DENSITIES: [(&str, i32); 3] = [
    ("Compact", design::SPACE_SM),
    ("Standard", design::SPACE_LG),
    ("Spacious", design::SPACE_XXL),
];

struct Lab {
    pads: Vec<(&'static str, &'static str)>,
    selected: Option<usize>,
    density: usize,
    legends: bool,
    enabled: bool,
    running: bool,
    level: f64,
    name: String,
}

impl Default for Lab {
    fn default() -> Self {
        Self {
            pads: PADS.to_vec(),
            selected: None,
            density: 1,
            legends: true,
            enabled: true,
            running: false,
            level: 0.62,
            name: "Studio matrix".into(),
        }
    }
}

impl Lab {
    fn selected_title(&self) -> String {
        self.selected
            .and_then(|i| self.pads.get(i).map(|p| p.0.to_string()))
            .unwrap_or_else(|| "No key selected".into())
    }
    fn reset(&mut self) {
        *self = Self::default();
    }
}

type LabRef = Rc<RefCell<Lab>>;

pub fn build(app: &Shared) -> gtk4::Widget {
    let root = design::pane_root();
    root.append(&design::section_title("Hardware, softened."));
    let cap = design::dim_label(
        "Lyra's Bubblegum keycaps — press the mock deck, then audit every control state in the component catalog.",
    );
    cap.set_wrap(true);
    cap.set_xalign(0.0);
    root.append(&cap);

    // ── header controls: segmented lab picker + theme pickers ─────────
    let lab = Rc::new(RefCell::new(Lab::default()));
    let stack = gtk4::Stack::new();
    stack.set_vexpand(true);
    stack.set_transition_type(gtk4::StackTransitionType::Crossfade);

    let switcher = gtk4::StackSwitcher::new();
    switcher.set_stack(Some(&stack));
    switcher.set_halign(gtk4::Align::Start);

    let controls = gtk4::Box::new(gtk4::Orientation::Horizontal, design::SPACE_LG);
    controls.append(&switcher);

    // Appearance — mirrors the Mac's segmented appearance picker.
    let appearance = gtk4::DropDown::from_strings(&["System", "Dark", "Light"]);
    appearance.set_tooltip_text(Some("App appearance"));
    appearance.set_selected(match app.prefs.borrow().appearance.as_str() {
        "dark" => 1,
        "light" => 2,
        _ => 0,
    });
    {
        let app = app.clone();
        appearance.connect_selected_notify(move |dd| {
            let sm = adw::StyleManager::default();
            let (scheme, pref) = match dd.selected() {
                1 => (adw::ColorScheme::ForceDark, "dark"),
                2 => (adw::ColorScheme::ForceLight, "light"),
                _ => (adw::ColorScheme::PreferDark, "system"),
            };
            sm.set_color_scheme(scheme);
            let data_dir = app.host.borrow().data_dir.clone();
            let mut p = app.prefs.borrow_mut();
            p.appearance = pref.to_string();
            p.save(&data_dir);
        });
    }
    controls.append(&appearance);

    // Palette — restamps the generated token set live.
    let palette = gtk4::DropDown::from_strings(&["Lavender", "Rose", "Mint"]);
    palette.set_tooltip_text(Some("Palette"));
    palette.set_selected(match app.prefs.borrow().palette.as_str() {
        "rose" => 1,
        "mint" => 2,
        _ => 0,
    });
    {
        let app = app.clone();
        palette.connect_selected_notify(move |dd| {
            let name = match dd.selected() {
                1 => "rose",
                2 => "mint",
                _ => "lavender",
            };
            let data_dir = app.host.borrow().data_dir.clone();
            {
                let mut p = app.prefs.borrow_mut();
                p.palette = name.to_string();
                p.save(&data_dir);
            }
            design::apply_theme(&app);
        });
    }
    controls.append(&palette);
    root.append(&controls);

    stack.add_titled(&canvas_tab(&lab), Some("canvas"), "Layout Canvas");
    stack.add_titled(&catalog_tab(), Some("components"), "Components");
    root.append(&stack);

    root.upcast()
}

// ── Layout canvas ────────────────────────────────────────────────────────

fn canvas_tab(lab: &LabRef) -> gtk4::Widget {
    let wrap = gtk4::ScrolledWindow::new();
    wrap.set_vexpand(true);
    let cols = gtk4::Box::new(gtk4::Orientation::Horizontal, design::SPACE_XL);
    cols.set_valign(gtk4::Align::Start);

    // ── matrix tray ──
    let matrix_card = design::card(gtk4::Orientation::Vertical);
    matrix_card.set_hexpand(true);
    let matrix_name = gtk4::Label::new(None);
    matrix_name.add_css_class("lyra-headline");
    matrix_name.set_xalign(0.0);
    matrix_card.append(&matrix_name);
    let selected_l = design::dim_label("");
    selected_l.set_xalign(0.0);
    matrix_card.append(&selected_l);

    let matrix = gtk4::FlowBox::new();
    matrix.set_min_children_per_line(3);
    matrix.set_max_children_per_line(3);
    matrix.set_selection_mode(gtk4::SelectionMode::None);
    matrix.set_halign(gtk4::Align::Center);
    matrix.set_homogeneous(true);
    matrix_card.append(&matrix);

    let run_btn = design::primary_button("Start sequence");
    run_btn.set_hexpand(true);
    matrix_card.append(&run_btn);
    let run_state = design::dim_label("Sequence idle");
    run_state.set_xalign(0.0);
    matrix_card.append(&run_state);
    let honest = design::dim_label("Local interaction demo — no audio is triggered.");
    honest.add_css_class("lyra-micro");
    honest.set_xalign(0.0);
    matrix_card.append(&honest);

    // ── inspector tray ──
    let insp = design::card(gtk4::Orientation::Vertical);
    insp.set_size_request(280, -1);
    insp.set_hexpand(false);
    let ih = gtk4::Label::new(Some("Layout controls"));
    ih.add_css_class("lyra-headline");
    ih.set_xalign(0.0);
    insp.append(&ih);

    let name_entry = gtk4::Entry::new();
    name_entry.set_placeholder_text(Some("Matrix name"));
    insp.append(&name_entry);

    let density = gtk4::DropDown::from_strings(&["Compact", "Standard", "Spacious"]);
    density.set_selected(lab.borrow().density as u32);
    density.set_tooltip_text(Some("Key spacing"));
    insp.append(&density);

    let (legends_row, legends_sw) = toggle_row("Show legends");
    let (enabled_row, enabled_sw) = toggle_row("Enable keys");
    insp.append(&legends_row);
    insp.append(&enabled_row);

    let moves = gtk4::Box::new(gtk4::Orientation::Horizontal, design::SPACE_SM);
    let move_l = design::secondary_button("Move left");
    let move_r = design::secondary_button("Move right");
    moves.append(&move_l);
    moves.append(&move_r);
    insp.append(&moves);

    let sel_caption = design::dim_label("Selected: No key selected");
    sel_caption.set_xalign(0.0);
    insp.append(&sel_caption);

    let level = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.01);
    level.set_value(lab.borrow().level);
    level.set_draw_value(false);
    insp.append(&level);
    let level_caption = design::dim_label("Demo level — local value only");
    level_caption.set_xalign(0.0);
    insp.append(&level_caption);

    let reset_btn = design::secondary_button("Reset layout");
    insp.append(&reset_btn);

    cols.append(&matrix_card);
    cols.append(&insp);
    wrap.set_child(Some(&cols));

    // ── wiring ──
    let pad_btns: Rc<RefCell<Vec<gtk4::Button>>> = Rc::new(RefCell::new(Vec::new()));
    let rebuild = {
        let lab = lab.clone();
        let matrix = matrix.clone();
        let selected_l = selected_l.clone();
        let sel_caption = sel_caption.clone();
        let move_l = move_l.clone();
        let move_r = move_r.clone();
        let matrix_name = matrix_name.clone();
        let pad_btns = pad_btns.clone();
        Rc::new(move || {
            let l = lab.borrow();
            while let Some(c) = matrix.first_child() {
                matrix.remove(&c);
            }
            pad_btns.borrow_mut().clear();
            let gap = DENSITIES[l.density].1;
            matrix.set_row_spacing(gap as u32);
            matrix.set_column_spacing(gap as u32);
            for (i, (title, icon)) in l.pads.iter().enumerate() {
                let cell = gtk4::Box::new(gtk4::Orientation::Vertical, design::SPACE_XS);
                let btn = gtk4::Button::new();
                btn.set_child(Some(&gtk4::Image::from_icon_name(icon)));
                btn.add_css_class("lyra-pad");
                btn.set_tooltip_text(Some(title));
                btn.set_sensitive(l.enabled);
                if l.selected == Some(i) {
                    btn.add_css_class("selected");
                }
                {
                    let lab = lab.clone();
                    let pad_btns = pad_btns.clone();
                    let sel_l = selected_l.clone();
                    let sel_c = sel_caption.clone();
                    let ml = move_l.clone();
                    let mr = move_r.clone();
                    btn.connect_clicked(move |b| {
                        {
                            let mut l = lab.borrow_mut();
                            if !l.enabled {
                                return;
                            }
                            l.selected = Some(i);
                        }
                        for pb in pad_btns.borrow().iter() {
                            pb.remove_css_class("selected");
                        }
                        b.add_css_class("selected");
                        let title = lab.borrow().selected_title();
                        sel_l.set_label(&title);
                        sel_c.set_label(&format!("Selected: {title}"));
                        update_moves(&lab.borrow(), &ml, &mr);
                    });
                }
                pad_btns.borrow_mut().push(btn.clone());
                cell.append(&btn);
                if l.legends {
                    let t = design::dim_label(title);
                    t.add_css_class("lyra-micro");
                    cell.append(&t);
                }
                matrix.insert(&cell, -1);
            }
            matrix_name.set_label(if l.name.trim().is_empty() {
                "Untitled matrix"
            } else {
                l.name.trim()
            });
            selected_l.set_label(&l.selected_title());
            sel_caption.set_label(&format!("Selected: {}", l.selected_title()));
            update_moves(&l, &move_l, &move_r);
        })
    };
    rebuild();

    fn update_moves(l: &Lab, l_btn: &gtk4::Button, r_btn: &gtk4::Button) {
        l_btn.set_sensitive(l.enabled && l.selected.map(|i| i > 0).unwrap_or(false));
        r_btn.set_sensitive(l.enabled && l.selected.map(|i| i + 1 < l.pads.len()).unwrap_or(false));
    }

    name_entry.set_text(&lab.borrow().name);
    {
        let lab = lab.clone();
        let rebuild = rebuild.clone();
        name_entry.connect_changed(move |e| {
            lab.borrow_mut().name = e.text().to_string();
            rebuild();
        });
    }
    {
        let lab = lab.clone();
        let rebuild = rebuild.clone();
        density.connect_selected_notify(move |dd| {
            lab.borrow_mut().density = dd.selected() as usize;
            rebuild();
        });
    }
    legends_sw.set_active(lab.borrow().legends);
    {
        let lab = lab.clone();
        let rebuild = rebuild.clone();
        legends_sw.connect_active_notify(move |s| {
            lab.borrow_mut().legends = s.is_active();
            rebuild();
        });
    }
    enabled_sw.set_active(lab.borrow().enabled);
    {
        let lab = lab.clone();
        let rebuild = rebuild.clone();
        enabled_sw.connect_active_notify(move |s| {
            lab.borrow_mut().enabled = s.is_active();
            rebuild();
        });
    }
    {
        let lab = lab.clone();
        let rebuild = rebuild.clone();
        move_l.connect_clicked(move |_| {
            let mut l = lab.borrow_mut();
            if let Some(i) = l.selected {
                if i > 0 {
                    l.pads.swap(i, i - 1);
                    l.selected = Some(i - 1);
                }
            }
            drop(l);
            rebuild();
        });
    }
    {
        let lab = lab.clone();
        let rebuild = rebuild.clone();
        move_r.connect_clicked(move |_| {
            let mut l = lab.borrow_mut();
            if let Some(i) = l.selected {
                if i + 1 < l.pads.len() {
                    l.pads.swap(i, i + 1);
                    l.selected = Some(i + 1);
                }
            }
            drop(l);
            rebuild();
        });
    }
    {
        let lab = lab.clone();
        let level_caption = level_caption.clone();
        level.connect_value_changed(move |s| {
            let v = s.value();
            lab.borrow_mut().level = v;
            level_caption.set_label(&format!("Demo level {:.0}% — local value only", v * 100.0));
        });
    }
    {
        let lab = lab.clone();
        let rebuild = rebuild.clone();
        let name_entry = name_entry.clone();
        let density = density.clone();
        let legends_sw = legends_sw.clone();
        let enabled_sw = enabled_sw.clone();
        let level = level.clone();
        reset_btn.connect_clicked(move |_| {
            lab.borrow_mut().reset();
            name_entry.set_text(&lab.borrow().name);
            density.set_selected(lab.borrow().density as u32);
            legends_sw.set_active(lab.borrow().legends);
            enabled_sw.set_active(lab.borrow().enabled);
            level.set_value(lab.borrow().level);
            rebuild();
        });
    }
    {
        let lab = lab.clone();
        let run_state = run_state.clone();
        let run_btn = run_btn.clone();
        run_btn.connect_clicked(move |b| {
            let mut l = lab.borrow_mut();
            if !l.enabled {
                return;
            }
            l.running = !l.running;
            b.set_label(if l.running {
                "Pause sequence"
            } else {
                "Start sequence"
            });
            run_state.set_label(if l.running {
                "Sequence active"
            } else {
                "Sequence idle"
            });
        });
    }

    wrap.upcast()
}

fn toggle_row(label: &str) -> (gtk4::Box, gtk4::Switch) {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, design::SPACE_SM);
    let l = gtk4::Label::new(Some(label));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    let sw = gtk4::Switch::new();
    sw.set_valign(gtk4::Align::Center);
    row.append(&l);
    row.append(&sw);
    (row, sw)
}

// ── Component catalog ────────────────────────────────────────────────────

fn catalog_tab() -> gtk4::Widget {
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, design::SPACE_XL);
    root.set_margin_top(design::SPACE_SM);
    root.set_margin_bottom(design::SPACE_SM);

    let h = gtk4::Label::new(Some("Component catalog"));
    h.add_css_class("lyra-headline");
    h.set_xalign(0.0);
    root.append(&h);
    let cap =
        design::dim_label("Simulated states — cells are inert snapshots, not working controls.");
    cap.set_xalign(0.0);
    root.append(&cap);

    let grid = gtk4::Grid::new();
    grid.set_row_spacing(design::SPACE_XL as u32);
    grid.set_column_spacing(design::SPACE_LG as u32);

    let states = ["Normal", "Hover", "Pressed", "Disabled"];
    grid.attach(&catalog_label("Component"), 0, 0, 1, 1);
    for (i, s) in states.iter().enumerate() {
        grid.attach(&catalog_label(s), i as i32 + 1, 0, 1, 1);
    }

    // rows of specimen builders
    type Specimen = (&'static str, Box<dyn Fn(&str) -> gtk4::Widget>);
    let specimens: Vec<Specimen> = vec![
        (
            "Circular key",
            Box::new(|state| {
                let b = gtk4::Button::new();
                b.set_child(Some(&gtk4::Image::from_icon_name("starred-symbolic")));
                b.add_css_class("lyra-key");
                sim(b.upcast_ref(), state)
            }),
        ),
        (
            "Pill key",
            Box::new(|state| {
                let b = gtk4::Button::with_label("Keycap");
                b.add_css_class("sharp");
                sim(b.upcast_ref(), state)
            }),
        ),
        (
            "Prominent pill",
            Box::new(|state| {
                let b = gtk4::Button::with_label("Primary");
                b.add_css_class("suggested-action");
                sim(b.upcast_ref(), state)
            }),
        ),
        (
            "Toggle",
            Box::new(|state| {
                let sw = gtk4::Switch::new();
                sw.set_valign(gtk4::Align::Start);
                if state == "Pressed" {
                    sw.set_active(true);
                }
                sim(sw.upcast_ref(), state)
            }),
        ),
        (
            "Text field",
            Box::new(|state| {
                let e = gtk4::Entry::new();
                e.set_text("Lyra");
                e.set_width_request(160);
                sim(e.upcast_ref(), state)
            }),
        ),
        (
            "Tray",
            Box::new(|state| {
                let tray = design::card(gtk4::Orientation::Vertical);
                let inner = gtk4::Button::new();
                inner.set_child(Some(&gtk4::Image::from_icon_name("emblem-system-symbolic")));
                inner.add_css_class("lyra-key");
                tray.append(&inner);
                let d = design::dim_label("Passive container — state is the child's");
                d.add_css_class("lyra-micro");
                d.set_wrap(true);
                d.set_max_width_chars(18);
                tray.append(&d);
                sim(tray.upcast_ref(), state)
            }),
        ),
    ];

    for (r, (name, make)) in specimens.iter().enumerate() {
        let l = gtk4::Label::new(Some(name));
        l.set_xalign(0.0);
        l.add_css_class("lyra-caption");
        grid.attach(&l, 0, r as i32 + 1, 1, 1);
        for (c, state) in states.iter().enumerate() {
            let cell = gtk4::Box::new(gtk4::Orientation::Vertical, design::SPACE_XS);
            let specimen = make(state);
            specimen.set_sensitive(state != &"Disabled");
            specimen.set_can_focus(false);
            specimen.set_can_target(false);
            cell.append(&specimen);
            let tag = design::dim_label(&format!("{state}, simulated"));
            tag.add_css_class("lyra-micro");
            tag.set_xalign(0.0);
            cell.append(&tag);
            grid.attach(&cell, c as i32 + 1, r as i32 + 1, 1, 1);
        }
    }
    root.append(&grid);
    root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // ── live controls — real events, not snapshots ──
    let lh = gtk4::Label::new(Some("Live controls"));
    lh.add_css_class("lyra-headline");
    lh.set_xalign(0.0);
    root.append(&lh);

    let presses = Rc::new(Cell::new(0u32));
    let live_on = Rc::new(Cell::new(false));

    let live_row = gtk4::Box::new(gtk4::Orientation::Horizontal, design::SPACE_LG);
    let press_btn = design::primary_button("Press me");
    press_btn.set_tooltip_text(Some("Real click — counts presses"));
    live_row.append(&press_btn);
    let press_count = design::dim_label("Pressed 0×");
    press_count.set_valign(gtk4::Align::Center);
    live_row.append(&press_count);
    let (toggle_row_w, live_sw) = toggle_row("Live toggle");
    live_row.append(&toggle_row_w);
    let field_col = gtk4::Box::new(gtk4::Orientation::Vertical, design::SPACE_XS);
    let fl = design::dim_label("Name");
    fl.set_xalign(0.0);
    let live_entry = gtk4::Entry::new();
    live_entry.set_text("Lyra");
    live_entry.set_width_request(180);
    live_entry.set_tooltip_text(Some("Type to edit — Tab focuses it"));
    field_col.append(&fl);
    field_col.append(&live_entry);
    live_row.append(&field_col);
    root.append(&live_row);

    let (enable_row, enable_sw) = toggle_row("Enable live samples");
    enable_sw.set_active(true);
    root.append(&enable_row);
    let echo = design::dim_label("Toggle is off · field says “Lyra”");
    echo.set_xalign(0.0);
    echo.set_wrap(true);
    root.append(&echo);

    {
        let press_count = press_count.clone();
        let presses = presses.clone();
        press_btn.connect_clicked(move |_| {
            presses.set(presses.get() + 1);
            press_count.set_label(&format!("Pressed {}×", presses.get()));
        });
    }
    {
        let echo2 = echo.clone();
        let live_on2 = live_on.clone();
        let live_entry2 = live_entry.clone();
        let update = move || {
            echo2.set_label(&format!(
                "Toggle is {} · field says “{}”",
                if live_on2.get() { "on" } else { "off" },
                live_entry2.text()
            ));
        };
        let update2 = update.clone();
        live_sw.connect_active_notify(move |s| {
            live_on.set(s.is_active());
            update();
        });
        live_entry.connect_changed(move |_| update2());
    }
    {
        let press_btn = press_btn.clone();
        let live_sw = live_sw.clone();
        let live_entry = live_entry.clone();
        enable_sw.connect_active_notify(move |s| {
            let on = s.is_active();
            press_btn.set_sensitive(on);
            live_sw.set_sensitive(on);
            live_entry.set_sensitive(on);
        });
    }

    scroll.set_child(Some(&root));
    scroll.upcast()
}

fn catalog_label(text: &str) -> gtk4::Label {
    let l = gtk4::Label::new(Some(text));
    l.add_css_class("lyra-caption");
    l.add_css_class("dim");
    l.set_xalign(0.0);
    l
}

/// Stamp the simulated-state class — `Disabled` is handled by the
/// caller via set_sensitive, which produces the real disabled visual.
fn sim(w: &gtk4::Widget, state: &str) -> gtk4::Widget {
    match state {
        "Hover" => w.add_css_class("sim-hover"),
        "Pressed" => w.add_css_class("sim-pressed"),
        _ => {}
    }
    w.clone()
}
