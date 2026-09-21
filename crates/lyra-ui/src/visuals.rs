//! Visuals pane — draws `LyraVizFrame` (the same 64-band/wave/meters
//! contract the SwiftUI renderers consume, docs/VIZ-CONTRACT.md) via
//! cairo on a tick-driven DrawingArea. Mode set covers the
//! information-bearing modes; decorative particle modes from the Swift
//! port collapse to the nearest equivalent here.

use crate::design;
use crate::{engine, ffi, Shared};
use gtk4::glib;
use gtk4::prelude::*;
use lyra_engine::VizFrame;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

const MODES: &[&str] = &[
    "Bars",
    "Wave",
    "Scope",
    "Pulse",
    "Stereo",
    "ClassicLED",
    "Spectrogram",
    "Heartbeat",
];

// lilac accent / mint secondary, matching theme.rs
const ACCENT: (f64, f64, f64) = (0.773, 0.702, 0.847);
const MINT: (f64, f64, f64) = (0.663, 0.788, 0.765);

struct Viz {
    last_seq: u64,
    /// band history for Spectrogram — ~120 columns of 64-band snapshots
    history: VecDeque<[f32; 64]>,
    /// beat envelope smoothing for Pulse/Heartbeat
    pulse: f64,
}

pub fn build(app: &Shared) -> gtk4::Widget {
    let root = design::pane_root();

    let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let t = design::section_title("Visuals");
    head.append(&t);
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    head.append(&spacer);
    let picker = gtk4::DropDown::from_strings(MODES);
    picker.set_selected(app.prefs.borrow().viz_mode.clamp(0, MODES.len() as i32 - 1) as u32);
    head.append(&picker);
    root.append(&head);

    let hint = design::dim_label(
        "bands/wave/meters come straight off the engine's viz tap — nothing is synthesized",
    );
    hint.set_xalign(0.0);
    root.append(&hint);

    let area = gtk4::DrawingArea::builder()
        .vexpand(true)
        .hexpand(true)
        .build();
    area.add_css_class("lyra-card");
    root.append(&area);

    let state = Rc::new(RefCell::new(Viz {
        last_seq: 0,
        history: VecDeque::with_capacity(160),
        pulse: 0.0,
    }));

    {
        let state = state.clone();
        let app = app.clone();
        area.set_draw_func(move |_, cr, w, h| {
            let mut st = state.borrow_mut();
            let f = ffi::viz_frame(engine());
            if f.seq != st.last_seq {
                st.last_seq = f.seq;
                st.history.push_back(f.bands);
                if st.history.len() > 160 {
                    st.history.pop_front();
                }
            }
            let (w, h) = (w as f64, h as f64);
            cr.set_source_rgb(0.11, 0.09, 0.14);
            let _ = cr.paint();
            let mode = app.prefs.borrow().viz_mode as usize;
            draw(cr, w, h, &f, &mut st, mode);
        });
    }

    {
        let app = app.clone();
        let data_dir = app.host.borrow().data_dir.clone();
        picker.connect_selected_notify(move |d| {
            let mut p = app.prefs.borrow_mut();
            p.viz_mode = d.selected() as i32;
            p.save(&data_dir);
        });
    }

    // ~60 fps redraws, only while the pane is on screen
    glib::timeout_add_local(Duration::from_millis(16), {
        let area = area.clone();
        let stack = app.stack.clone();
        move || {
            if stack.visible_child_name().map(|n| n == "visuals") == Some(true) {
                area.queue_draw();
            }
            glib::ControlFlow::Continue
        }
    });

    root.upcast()
}

fn draw(cr: &gtk4::cairo::Context, w: f64, h: f64, f: &VizFrame, st: &mut Viz, mode: usize) {
    match mode {
        1 => draw_wave(cr, w, h, f),
        2 => draw_scope(cr, w, h, f),
        3 => draw_pulse(cr, w, h, f, st),
        4 => draw_stereo(cr, w, h, f),
        5 => draw_led(cr, w, h, f),
        6 => draw_spectrogram(cr, w, h, st),
        7 => draw_heartbeat(cr, w, h, f, st),
        _ => draw_bars(cr, w, h, f),
    }
}

fn draw_bars(cr: &gtk4::cairo::Context, w: f64, h: f64, f: &VizFrame) {
    let n = f.bands.len();
    let bw = w / n as f64;
    cr.set_source_rgb(ACCENT.0, ACCENT.1, ACCENT.2);
    for (i, b) in f.bands.iter().enumerate() {
        let bh = (*b as f64).clamp(0.0, 1.0) * (h - 8.0);
        cr.rectangle(i as f64 * bw + 1.0, h - bh, bw - 2.0, bh);
        let _ = cr.fill();
    }
    if f.clip != 0 {
        cr.set_source_rgb(0.9, 0.4, 0.4);
        cr.rectangle(w - 14.0, 4.0, 10.0, 10.0);
        let _ = cr.fill();
    }
}

fn draw_wave(cr: &gtk4::cairo::Context, w: f64, h: f64, f: &VizFrame) {
    for (ch, (wave, rgb)) in [f.wave_l, f.wave_r].iter().zip([ACCENT, MINT]).enumerate() {
        let _ = ch;
        cr.set_source_rgb(rgb.0, rgb.1, rgb.2);
        cr.set_line_width(1.5);
        let n = wave.len();
        let mid = h / 2.0 + (ch as f64 - 0.5) * h * 0.4;
        for (i, v) in wave.iter().enumerate() {
            let x = i as f64 / n as f64 * w;
            let y = mid + (*v as f64).clamp(-1.0, 1.0) * h * 0.22;
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        let _ = cr.stroke();
    }
}

/// Lissajous scope: wave_l on x, wave_r on y.
fn draw_scope(cr: &gtk4::cairo::Context, w: f64, h: f64, f: &VizFrame) {
    cr.set_source_rgb(MINT.0, MINT.1, MINT.2);
    cr.set_line_width(1.0);
    let (cx, cy, r) = (w / 2.0, h / 2.0, w.min(h) * 0.45);
    for i in 0..f.wave_l.len() {
        let x = cx + f.wave_l[i] as f64 * r;
        let y = cy - f.wave_r[i] as f64 * r;
        if i == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();
}

/// Beat pulse — ring expands on `beat`, brightness follows `level`.
fn draw_pulse(cr: &gtk4::cairo::Context, w: f64, h: f64, f: &VizFrame, st: &mut Viz) {
    st.pulse = (st.pulse * 0.86).max(f.beat as f64);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let base = w.min(h) * 0.16;
    let r = base * (1.0 + st.pulse * 0.8);
    cr.set_source_rgba(ACCENT.0, ACCENT.1, ACCENT.2, 0.25 + f.level as f64 * 0.6);
    cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
    cr.set_source_rgb(ACCENT.0, ACCENT.1, ACCENT.2);
    cr.set_line_width(2.0);
    cr.arc(
        cx,
        cy,
        r + 6.0 + f.bass as f64 * 10.0,
        0.0,
        std::f64::consts::TAU,
    );
    let _ = cr.stroke();
}

/// Classic VU — peak + RMS needles per channel.
fn draw_stereo(cr: &gtk4::cairo::Context, w: f64, h: f64, f: &VizFrame) {
    let rows = [
        ("L peak", f.peak[0]),
        ("L rms", f.rms[0]),
        ("R peak", f.peak[1]),
        ("R rms", f.rms[1]),
    ];
    let rh = h / rows.len() as f64;
    cr.select_font_face(
        "Sans",
        gtk4::cairo::FontSlant::Normal,
        gtk4::cairo::FontWeight::Normal,
    );
    cr.set_font_size(12.0);
    for (i, (label, v)) in rows.iter().enumerate() {
        let y = i as f64 * rh + rh * 0.3;
        cr.set_source_rgb(0.55, 0.51, 0.66);
        cr.move_to(10.0, y + 4.0);
        let _ = cr.show_text(label);
        let bx = 80.0;
        let bw = w - bx - 20.0;
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.08);
        cr.rectangle(bx, y - 6.0, bw, 14.0);
        let _ = cr.fill();
        cr.set_source_rgb(MINT.0, MINT.1, MINT.2);
        cr.rectangle(bx, y - 6.0, bw * (*v as f64).clamp(0.0, 1.0), 14.0);
        let _ = cr.fill();
        // clip marker per channel
        if (f.clip >> (i / 2)) & 1 == 1 && i % 2 == 0 {
            cr.set_source_rgb(0.9, 0.4, 0.4);
            cr.rectangle(bx + bw - 4.0, y - 6.0, 4.0, 14.0);
            let _ = cr.fill();
        }
    }
}

/// LED columns — bands quantized into segment stacks.
fn draw_led(cr: &gtk4::cairo::Context, w: f64, h: f64, f: &VizFrame) {
    let n = 32usize;
    let segs = 16;
    let bw = w / n as f64;
    let sh = h / segs as f64;
    for i in 0..n {
        let v = f.bands[i * 2] as f64;
        let lit = (v * segs as f64) as i32;
        for s in 0..segs {
            let on = s < lit;
            let hot = s > (segs as f64 * 0.8) as i32;
            let rgb = if !on {
                (0.16, 0.14, 0.20)
            } else if hot {
                (0.9, 0.45, 0.45)
            } else {
                MINT
            };
            cr.set_source_rgb(rgb.0, rgb.1, rgb.2);
            cr.rectangle(
                i as f64 * bw + 1.0,
                h - (s + 1) as f64 * sh + 1.0,
                bw - 2.0,
                sh - 2.0,
            );
            let _ = cr.fill();
        }
    }
}

/// Scrolling heat of band history.
fn draw_spectrogram(cr: &gtk4::cairo::Context, w: f64, h: f64, st: &Viz) {
    let cols = st.history.len().max(1);
    let cw = w / 160.0;
    let bh = h / 64.0;
    for (x, bands) in st.history.iter().enumerate() {
        let _ = cols;
        for (i, b) in bands.iter().enumerate() {
            let v = (*b as f64).clamp(0.0, 1.0);
            if v < 0.02 {
                continue;
            }
            cr.set_source_rgba(ACCENT.0, ACCENT.1, ACCENT.2, v);
            cr.rectangle(
                x as f64 * cw,
                h - (i + 1) as f64 * bh,
                cw.max(1.0),
                bh.max(1.0),
            );
            let _ = cr.fill();
        }
    }
}

/// ECG-style trace — wave midline, amplitude modulated by beat.
fn draw_heartbeat(cr: &gtk4::cairo::Context, w: f64, h: f64, f: &VizFrame, st: &mut Viz) {
    st.pulse = (st.pulse * 0.9).max(f.beat as f64);
    let amp = h * (0.15 + st.pulse * 0.3);
    cr.set_source_rgb(0.9, 0.55, 0.6);
    cr.set_line_width(2.0);
    let n = f.wave_l.len();
    for (i, v) in f.wave_l.iter().enumerate() {
        let x = i as f64 / n as f64 * w;
        let y = h / 2.0 + (*v as f64) * amp;
        if i == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();
}
