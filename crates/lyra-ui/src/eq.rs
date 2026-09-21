//! Equalizer pane — the app's 10-band ISO-center parametric, drawn as
//! vertical gain sliders plus the live magnitude response from the
//! engine (`lyra_engine_eq_response` — same biquads as the audio path).

use crate::model::EQ_FREQS;
use crate::{engine, ffi, Shared};
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const DB_RANGE: f64 = 24.0; // ±12 dB like the app

pub fn build(app: &Shared) -> gtk4::Widget {
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(20);
    root.set_margin_end(20);

    let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let t = gtk4::Label::new(Some("Equalizer"));
    t.add_css_class("title-1");
    head.append(&t);
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    head.append(&spacer);
    let reset = gtk4::Button::with_label("Reset");
    reset.add_css_class("sharp");
    head.append(&reset);
    root.append(&head);

    // response curve on top
    let curve = gtk4::DrawingArea::builder()
        .height_request(160)
        .hexpand(true)
        .build();
    curve.add_css_class("lyra-card");
    root.append(&curve);

    // sliders
    let sliders = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    sliders.set_homogeneous(true);
    sliders.set_vexpand(true);
    let gains = Rc::new(RefCell::new([0.0f64; 10]));
    let mut scales = Vec::new();
    for (i, f) in EQ_FREQS.iter().enumerate() {
        let col = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        let val = gtk4::Label::new(Some("0.0"));
        val.add_css_class("dim");
        let s = gtk4::Scale::with_range(
            gtk4::Orientation::Vertical,
            -DB_RANGE / 2.0,
            DB_RANGE / 2.0,
            0.5,
        );
        s.set_inverted(true);
        s.set_vexpand(true);
        s.add_css_class("eq");
        s.set_value(0.0);
        let freq_txt = if *f >= 1000.0 {
            format!("{}k", (*f / 1000.0) as u32)
        } else {
            format!("{}", *f as u32)
        };
        let freq_l = gtk4::Label::new(Some(&freq_txt));
        freq_l.add_css_class("dim");
        col.append(&val);
        col.append(&s);
        col.append(&freq_l);
        sliders.append(&col);
        scales.push((s.clone(), val.clone()));

        let gains = gains.clone();
        let curve = curve.clone();
        s.connect_value_changed(move |sc| {
            let g = sc.value();
            gains.borrow_mut()[i] = g;
            val.set_label(&format!("{g:.1}"));
            // applyEQ: freq + fixed Q, peaking everywhere but band 0 (low-shelf)
            ffi::set_band(engine(), i as i32, EQ_FREQS[i], 0.9, g as f32, i != 0);
            curve.queue_draw();
        });
    }
    root.append(&sliders);

    {
        let gains = gains.clone();
        let curve = curve.clone();
        let scales: Vec<(gtk4::Scale, gtk4::Label)> = scales;
        reset.connect_clicked(move |_| {
            gains.borrow_mut().fill(0.0);
            for (s, v) in &scales {
                s.set_value(0.0);
                v.set_label("0.0");
            }
            curve.queue_draw();
        });
    }

    // draw the engine's response curve
    {
        curve.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (w as f64, h as f64);
            cr.set_source_rgb(0.17, 0.14, 0.22);
            let _ = cr.paint();
            // gridlines at ±6 dB and unity
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.08);
            for frac in [0.25f64, 0.5, 0.75] {
                cr.move_to(0.0, h * frac);
                cr.line_to(w, h * frac);
                let _ = cr.stroke();
            }
            let resp = ffi::eq_response(crate::engine());
            let (Some(freqs), Some(db)) = (
                resp.get("freqs").and_then(|v| v.as_array()),
                resp.get("db").and_then(|v| v.as_array()),
            ) else {
                return;
            };
            if freqs.is_empty() || db.is_empty() {
                return;
            }
            // log-x like the audio path (20 Hz–20 kHz), ±12 dB y
            let lx = |f: f64| (f.ln() - 20f64.ln()) / (20000f64.ln() - 20f64.ln()) * w;
            cr.set_source_rgb(0.77, 0.70, 0.85);
            cr.set_line_width(2.0);
            for (i, (f, d)) in freqs.iter().zip(db.iter()).enumerate() {
                let (Some(f), Some(d)) = (f.as_f64(), d.as_f64()) else {
                    continue;
                };
                let y = h * 0.5 - (d / (DB_RANGE / 2.0)) * (h * 0.45);
                if i == 0 {
                    cr.move_to(lx(f), y.clamp(0.0, h));
                } else {
                    cr.line_to(lx(f), y.clamp(0.0, h));
                }
            }
            let _ = cr.stroke();
        });
    }

    // refresh curve ~8 Hz while the pane is visible
    glib::timeout_add_local(Duration::from_millis(125), {
        let curve = curve.clone();
        let stack = app.stack.clone();
        move || {
            if stack.visible_child_name().map(|n| n == "eq") == Some(true) {
                curve.queue_draw();
            }
            glib::ControlFlow::Continue
        }
    });

    root.upcast()
}
