//! Map pane — offline analysis of the playing/selected track via
//! `lyra_map_analyze`/`lyra_map_for_track`, rendered as the app does:
//! tuning + grid summary, section list, and a chord-timeline strip.

use crate::design;
use crate::{ffi, Msg, Shared};
use gtk4::prelude::*;
use lyra_map::SongMap;
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use std::thread;

struct MapState {
    map: Option<SongMap>,
    title: String,
    note: String,
    busy: bool,
}

const PC: [&str; 12] = [
    "C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B",
];

fn chord_name(c: &lyra_map::ChordEvent) -> String {
    match c.root {
        None => "N.C.".into(),
        Some(r) => {
            let q = match c.quality {
                lyra_map::ChordQuality::Maj => "",
                lyra_map::ChordQuality::Min => "m",
                lyra_map::ChordQuality::Nc => "",
            };
            match c.bass.filter(|b| Some(*b) != c.root) {
                Some(b) => format!("{}{}/{}", PC[r as usize % 12], q, PC[b as usize % 12]),
                None => format!("{}{}", PC[r as usize % 12], q),
            }
        }
    }
}

fn tuning_name(m: &SongMap) -> String {
    let names: Vec<String> = m
        .tuning
        .strings
        .iter()
        .map(|s| PC[(*s as usize).rem_euclid(12)].to_string())
        .collect();
    let capo = if m.tuning.capo > 0 {
        format!(" · capo {}", m.tuning.capo)
    } else {
        String::new()
    };
    format!(
        "{}{} · conf {:.0}%",
        names.join(" "),
        capo,
        m.tuning.conf * 100.0
    )
}

pub fn build(app: &Shared) -> gtk4::Widget {
    let root = design::pane_root();

    let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let t = design::section_title("Map");
    head.append(&t);
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    head.append(&spacer);
    let analyze_btn = design::primary_button("Analyze current track");
    head.append(&analyze_btn);
    root.append(&head);

    let note_l =
        design::dim_label("Analyze a library track for tuning, beat grid, sections and chords.");
    note_l.set_xalign(0.0);
    root.append(&note_l);

    let summary = gtk4::Label::new(None);
    summary.set_xalign(0.0);
    root.append(&summary);

    // chord timeline strip
    let strip = gtk4::DrawingArea::builder()
        .height_request(120)
        .hexpand(true)
        .build();
    strip.add_css_class("lyra-card");
    root.append(&strip);

    // sections + notes readout
    let detail = design::dim_label("");
    detail.set_xalign(0.0);
    detail.set_valign(gtk4::Align::Start);
    detail.set_selectable(true);
    let scroll = gtk4::ScrolledWindow::builder()
        .child(&detail)
        .vexpand(true)
        .build();
    root.append(&scroll);

    let st = Rc::new(RefCell::new(MapState {
        map: None,
        title: String::new(),
        note: String::new(),
        busy: false,
    }));

    // render hook for handle_msg → MapDone
    {
        let st = st.clone();
        let summary = summary.clone();
        let detail = detail.clone();
        let strip = strip.clone();
        let note_l = note_l.clone();
        let analyze_btn = analyze_btn.clone();
        MAP_RENDER.with(|r| {
            *r.borrow_mut() = Some(Box::new(move |v: Value| {
                let mut s = st.borrow_mut();
                s.busy = false;
                analyze_btn.set_sensitive(true);
                analyze_btn.set_label("Analyze current track");
                if v.get("error").is_some() || v.is_null() {
                    note_l.set_label(
                        "analysis failed — local files only, pipeline needs a decodable track",
                    );
                    s.map = None;
                    summary.set_label("");
                    detail.set_label("");
                    strip.queue_draw();
                    return;
                }
                match serde_json::from_value::<SongMap>(v) {
                    Ok(m) => {
                        summary.set_label(&format!(
                            "{} · {:.0}s · {} chords · {} notes · {}",
                            s.title,
                            m.duration_s,
                            m.chords.len(),
                            m.notes.len(),
                            tuning_name(&m)
                        ));
                        let mut txt = String::new();
                        for (i, sec) in m.sections.iter().enumerate() {
                            txt.push_str(&format!(
                                "§{i}  {:>7.1}s–{:<7.1}s  {:?}  conf {:.0}%\n",
                                sec.t0,
                                sec.t1,
                                sec.label,
                                sec.conf * 100.0
                            ));
                        }
                        detail.set_label(&txt);
                        s.map = Some(m);
                    }
                    Err(e) => {
                        note_l.set_label(&format!("map parse: {e}"));
                        s.map = None;
                    }
                }
                strip.queue_draw();
            }))
        });
    }

    strip.set_draw_func({
        let st = st.clone();
        move |_, cr, w, h| {
            let (w, h) = (w as f64, h as f64);
            cr.set_source_rgb(0.11, 0.09, 0.14);
            let _ = cr.paint();
            let Some(m) = &st.borrow().map else { return };
            if m.duration_s <= 0.0 {
                return;
            }
            let x = |t: f32| t as f64 / m.duration_s as f64 * w;
            cr.select_font_face(
                "Sans",
                gtk4::cairo::FontSlant::Normal,
                gtk4::cairo::FontWeight::Bold,
            );
            cr.set_font_size(13.0);
            for (i, c) in m.chords.iter().enumerate() {
                let (x0, x1) = (x(c.t0), x(c.t1));
                if x1 - x0 < 2.0 {
                    continue;
                }
                // alternate row tint by root pc; alpha by confidence
                let pc = c.root.unwrap_or(0) as f64 / 12.0;
                let alpha = (0.25 + c.conf as f64 * 0.75).min(1.0);
                cr.set_source_rgba(0.5 + pc * 0.3, 0.55, 0.7 - pc * 0.2, alpha * 0.35);
                cr.rectangle(x0, 8.0, (x1 - x0).max(1.0), h - 36.0);
                let _ = cr.fill();
                if x1 - x0 > 28.0 {
                    cr.set_source_rgb(0.93, 0.90, 0.95);
                    cr.move_to(x0 + 4.0, h - 16.0);
                    let _ = cr.show_text(&chord_name(c));
                }
                let _ = i;
            }
            // strum ticks along the baseline
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
            for s in &m.strums {
                let sx = x(s.t);
                cr.move_to(sx, h - 8.0);
                cr.line_to(sx, h - 2.0);
            }
            let _ = cr.stroke();
        }
    });

    analyze_btn.connect_clicked({
        let app = app.clone();
        let st = st.clone();
        let analyze_btn = analyze_btn.clone();
        move |b| {
            let _ = b;
            let cur = app.host.borrow().current.clone();
            let Some(path) = cur else {
                st.borrow_mut().note = "play a track first".into();
                note_l.set_label("play a track first — map analyzes what's loaded");
                return;
            };
            if path.contains("://") {
                note_l.set_label("remote/torrent analysis isn't bridged — local files only");
                return;
            }
            st.borrow_mut().busy = true;
            st.borrow_mut().title = path.clone();
            analyze_btn.set_sensitive(false);
            analyze_btn.set_label("Analyzing…");
            let db = app.host.borrow().db_path.clone();
            let maps_dir = app.host.borrow().data_dir.join("maps");
            let tx = app.tx.clone();
            note_l.set_label("analyzing…");
            thread::spawn(move || {
                let _ = std::fs::create_dir_all(&maps_dir);
                // cached first, full pipeline otherwise
                let v = ffi::map_for_track(&db, &path);
                let v = if v.is_null() || v.get("error").is_some() {
                    ffi::map_analyze(&db, &path, &maps_dir, None)
                } else {
                    v
                };
                let _ = tx.send(Msg::MapDone(v));
            });
        }
    });

    let _ = &st;
    root.upcast()
}

type RenderFn = Box<dyn Fn(Value)>;
thread_local! {
    /// handle_msg → pane render hook; single pane so a thread-local slot
    /// is enough (panes outlive the session anyway).
    static MAP_RENDER: RefCell<Option<RenderFn>> = const { RefCell::new(None) };
}

pub fn on_map(_app: &Shared, v: Value) {
    MAP_RENDER.with(|r| {
        if let Some(f) = r.borrow().as_ref() {
            f(v);
        }
    });
}
