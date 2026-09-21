//! Coach pane — the app's strum-on-the-grid practice lane. The SwiftUI
//! side feeds `lyra_coach_push` from an AVAudioEngine tap; here the same
//! push runs from a cpal input stream (ALSA/JACK under the hood), so the
//! live session/judge is literally the same code path.

use crate::{ffi, Shared};
use gtk4::prelude::*;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

struct Coach {
    on: bool,
    verdicts: gtk4::ListBox,
    score_l: gtk4::Label,
    note_l: gtk4::Label,
    start_btn: gtk4::Button,
    bpm: gtk4::SpinButton,
    feedback: gtk4::DropDown,
    stream: Option<cpal::Stream>,
    seen: usize,
}

thread_local! {
    static COACH: RefCell<Option<Rc<RefCell<Coach>>>> = const { RefCell::new(None) };
}

pub fn build(_app: &Shared) -> gtk4::Widget {
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(20);
    root.set_margin_end(20);

    let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let t = gtk4::Label::new(Some("Coach"));
    t.add_css_class("title-1");
    head.append(&t);
    root.append(&head);

    let desc = gtk4::Label::new(Some(
        "Strum-on-the-grid: plays a metronome grid and grades your strums through the mic.",
    ));
    desc.set_xalign(0.0);
    desc.set_wrap(true);
    desc.add_css_class("dim");
    root.append(&desc);

    let ctl = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    ctl.add_css_class("lyra-card");
    ctl.append(&gtk4::Label::new(Some("BPM")));
    let bpm = gtk4::SpinButton::with_range(30.0, 400.0, 1.0);
    bpm.set_value(96.0);
    ctl.append(&bpm);
    let feedback = gtk4::DropDown::from_strings(&["full", "coarse", "end_of_phrase", "silent"]);
    ctl.append(&gtk4::Label::new(Some("Feedback")));
    ctl.append(&feedback);
    let start_btn = gtk4::Button::with_label("Start practice");
    start_btn.add_css_class("suggested-action");
    ctl.append(&start_btn);
    let cal_btn = gtk4::Button::with_label("Calibrate input");
    cal_btn.add_css_class("sharp");
    ctl.append(&cal_btn);
    root.append(&ctl);

    let note_l = gtk4::Label::new(None);
    note_l.set_xalign(0.0);
    note_l.add_css_class("dim");
    root.append(&note_l);

    let score_l = gtk4::Label::new(None);
    score_l.set_xalign(0.0);
    score_l.add_css_class("heading");
    root.append(&score_l);

    let verdicts = gtk4::ListBox::new();
    verdicts.set_selection_mode(gtk4::SelectionMode::None);
    let scroll = gtk4::ScrolledWindow::builder()
        .child(&verdicts)
        .vexpand(true)
        .build();
    root.append(&scroll);

    let c = Rc::new(RefCell::new(Coach {
        on: false,
        verdicts: verdicts.clone(),
        score_l: score_l.clone(),
        note_l: note_l.clone(),
        start_btn: start_btn.clone(),
        bpm: bpm.clone(),
        feedback: feedback.clone(),
        stream: None,
        seen: 0,
    }));
    COACH.with(|s| *s.borrow_mut() = Some(c.clone()));

    start_btn.connect_clicked({
        let c = c.clone();
        move |_| {
            let mut cc = c.borrow_mut();
            if cc.on {
                stop(&mut cc);
                return;
            }
            let bpm = cc.bpm.value();
            // same chart the app builds: 2 s lead-in, then 32 beats
            let beat = 60.0 / bpm;
            let chart: Vec<Value> = (0..32)
                .map(|i| json!({"t_secs": 2.0 + i as f64 * beat}))
                .collect();
            if !ffi::coach_new(&json!(chart).to_string(), "{}") {
                cc.note_l.set_label("coach init failed");
                return;
            }
            let mode = match cc.feedback.selected() {
                1 => "coarse",
                2 => "end_of_phrase",
                3 => "silent",
                _ => "full",
            };
            ffi::coach_feedback(mode);
            match start_input() {
                Ok(stream) => {
                    cc.stream = Some(stream);
                    cc.on = true;
                    cc.seen = 0;
                    cc.start_btn.set_label("Stop");
                    cc.note_l
                        .set_label(&format!("strum on the grid — {:.0} bpm", bpm));
                }
                Err(e) => {
                    ffi::coach_stop();
                    cc.note_l
                        .set_label(&format!("input tap failed — {e} (check mic/permissions)"));
                }
            }
        }
    });

    cal_btn.connect_clicked({
        let c = c.clone();
        move |_| {
            let cc = c.borrow();
            if !cc.on {
                cc.note_l
                    .set_label("start practice first — calibration needs the input tap");
                return;
            }
            ffi::coach_start_calibration(0.0, cc.bpm.value());
            cc.note_l
                .set_label("calibrating — strum each beat for ~8 beats");
        }
    });

    root.upcast()
}

/// cpal input tap → mono f32 → lyra_coach_push. `t_first` = stream-clock
/// seconds of the block's first sample, same contract as the AVAudioEngine
/// tap (sampleTime/sampleRate).
fn start_input() -> Result<cpal::Stream, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let dev = host
        .default_input_device()
        .ok_or_else(|| "no input device".to_string())?;
    let cfg = dev
        .default_input_config()
        .map_err(|e| format!("no input config: {e}"))?;
    let rate = cfg.sample_rate() as f64;
    let chans = cfg.channels() as usize;
    let n_seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mk_cb = |n: std::sync::Arc<std::sync::atomic::AtomicU64>| {
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            let t_first = n.load(std::sync::atomic::Ordering::Relaxed) as f64 / rate / chans as f64;
            n.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
            // downmix to mono
            let mono: Vec<f32> = data
                .chunks(chans)
                .map(|f| f.iter().sum::<f32>() / chans as f32)
                .collect();
            ffi::coach_push(&mono, t_first);
        }
    };
    let err = |e: cpal::Error| tracing::warn!("coach input: {e}");
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => dev.build_input_stream(cfg.config(), mk_cb(n_seen), err, None),
        cpal::SampleFormat::I16 => {
            let n2 = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            dev.build_input_stream(
                cfg.config(),
                move |data: &[i16], _| {
                    let t_first =
                        n2.load(std::sync::atomic::Ordering::Relaxed) as f64 / rate / chans as f64;
                    n2.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    let mono: Vec<f32> = data
                        .chunks(chans)
                        .map(|f| {
                            f.iter().map(|s| *s as f32 / i16::MAX as f32).sum::<f32>()
                                / chans as f32
                        })
                        .collect();
                    ffi::coach_push(&mono, t_first);
                },
                err,
                None,
            )
        }
        _ => return Err("unsupported input sample format".into()),
    }
    .map_err(|e| format!("{e}"))?;
    stream.play().map_err(|e| format!("{e}"))?;
    Ok(stream)
}

fn stop(c: &mut Coach) {
    ffi::coach_stop();
    c.stream = None;
    c.on = false;
    c.start_btn.set_label("Start practice");
    c.note_l.set_label("");
}

/// 150 ms tick — drains staged events + score (UI-thread polling, same
/// as the app's pollCoach).
pub fn poll(_app: &Shared) {
    COACH.with(|s| {
        let Some(c) = s.borrow().as_ref().cloned() else {
            return;
        };
        let mut cc = c.borrow_mut();
        if !cc.on {
            return;
        }
        for e in ffi::coach_events().as_array().cloned().unwrap_or_default() {
            match e.get("type").and_then(|t| t.as_str()) {
                Some("verdict") => {
                    let grade = e.get("grade").and_then(|g| g.as_str()).unwrap_or("?");
                    let ms = e
                        .get("error_ms")
                        .and_then(|m| m.as_f64())
                        .map(|m| format!("{:+.0} ms", m))
                        .unwrap_or_default();
                    let row = gtk4::Label::new(Some(&format!("{grade} {ms}")));
                    row.set_xalign(0.0);
                    cc.verdicts.append(&row);
                    // keep last 24
                    cc.seen += 1;
                    while cc.seen > 24 {
                        if let Some(r) = cc.verdicts.first_child() {
                            cc.verdicts.remove(&r);
                        }
                        cc.seen -= 1;
                    }
                }
                Some("calibration") => {
                    if let Some(off) = e.get("offset_ms").and_then(|o| o.as_f64()) {
                        cc.note_l
                            .set_label(&format!("calibrated — input latency {:.0} ms", off));
                    }
                }
                _ => {}
            }
        }
        let score = ffi::coach_score();
        if !score.is_null() {
            let acc = score.get("accuracy").and_then(|a| a.as_f64());
            let n = score.get("graded").and_then(|a| a.as_u64()).unwrap_or(0);
            cc.score_l.set_label(&match acc {
                Some(a) => format!("{:.0}% over {n} strums", a * 100.0),
                None => format!("{n} strums"),
            });
        }
    });
}
