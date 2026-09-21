//! Discover pane — lyra-search over legal torrent indexes, lossless-first.
//! Mirrors `runDiscover`/`expandDiscover`/`addDiscover`: search off-main,
//! sort lossless-then-seeds, expand resolves files + an addable spec,
//! Add hands the magnet to the torrent session.

use crate::{ffi, Msg, Shared};
use gtk4::prelude::*;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::thread;

struct Disc {
    list: gtk4::ListBox,
    status: gtk4::Label,
    busy: gtk4::Spinner,
    search_btn: gtk4::Button,
    results: Vec<Value>,
    /// resolved detail per result id — {files, addable}
    detail: std::collections::HashMap<usize, Value>,
    expanded: Option<usize>,
}

pub fn build(app: &Shared) -> gtk4::Widget {
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(20);
    root.set_margin_end(20);

    let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let t = gtk4::Label::new(Some("Discover"));
    t.add_css_class("title-1");
    head.append(&t);
    let entry = gtk4::SearchEntry::builder()
        .placeholder_text("Search legal indexes — artist, album…")
        .hexpand(true)
        .build();
    head.append(&entry);
    let lossy = gtk4::CheckButton::with_label("include lossy");
    head.append(&lossy);
    let search_btn = gtk4::Button::with_label("Search");
    search_btn.add_css_class("suggested-action");
    head.append(&search_btn);
    root.append(&head);

    let status_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let busy = gtk4::Spinner::new();
    let status = gtk4::Label::new(None);
    status.set_xalign(0.0);
    status.add_css_class("dim");
    status_row.append(&busy);
    status_row.append(&status);
    root.append(&status_row);

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.set_activate_on_single_click(true);
    let scroll = gtk4::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .build();
    root.append(&scroll);

    let st = Rc::new(RefCell::new(Disc {
        list: list.clone(),
        status: status.clone(),
        busy: busy.clone(),
        search_btn: search_btn.clone(),
        results: Vec::new(),
        detail: std::collections::HashMap::new(),
        expanded: None,
    }));

    DISC.with(|d| *d.borrow_mut() = Some(st.clone()));

    // activation on a row expands it (resolve); button inside handles Add
    let run_search = {
        let app = app.clone();
        let entry = entry.clone();
        let lossy = lossy.clone();
        let st = st.clone();
        move || {
            let q = entry.text().trim().to_string();
            if q.is_empty() {
                return;
            }
            st.borrow_mut().search_btn.set_sensitive(false);
            st.borrow_mut().search_btn.set_label("Searching…");
            st.borrow().busy.start();
            st.borrow().status.set_label("");
            let tx = app.tx.clone();
            let data_dir = app.host.borrow().data_dir.clone();
            let strict = !lossy.is_active();
            thread::spawn(move || {
                let resp = ffi::SearchHandle::open(&data_dir)
                    .map(|s| {
                        s.search(&json!({"text": q, "strict": strict, "limit": 25}).to_string())
                    })
                    .unwrap_or(Value::Null);
                let _ = tx.send(Msg::SearchDone(resp));
            });
        }
    };
    {
        let rs = run_search.clone();
        search_btn.connect_clicked(move |_| rs());
        entry.connect_activate(move |_| run_search());
    }

    // click a row → resolve (expand); the resolved box holds the Add btn
    list.connect_row_activated({
        let app = app.clone();
        let st = st.clone();
        move |_, row| {
            let idx = row.index();
            if idx < 0 {
                return;
            }
            let idx = idx as usize;
            {
                let mut s = st.borrow_mut();
                if s.expanded == Some(idx) {
                    s.expanded = None;
                    render_rows(&app, &st);
                    return;
                }
                s.expanded = Some(idx);
            }
            render_rows(&app, &st);
            if st.borrow().detail.contains_key(&idx) {
                return;
            }
            let Some(result) = st.borrow().results.get(idx).cloned() else {
                return;
            };
            let tx = app.tx.clone();
            let data_dir = app.host.borrow().data_dir.clone();
            thread::spawn(move || {
                let v = ffi::SearchHandle::open(&data_dir)
                    .map(|s| s.resolve(&result.to_string()))
                    .unwrap_or(Value::Null);
                let _ = tx.send(Msg::Resolved(idx, v));
            });
        }
    });

    root.upcast()
}

pub fn on_results(app: &Shared, resp: Value) {
    DISC.with(|d| {
        let Some(st) = d.borrow().as_ref().cloned() else {
            return;
        };
        {
            let mut s = st.borrow_mut();
            s.busy.stop();
            s.search_btn.set_sensitive(true);
            s.search_btn.set_label("Search");
            let mut results: Vec<Value> = resp
                .get("results")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            // lossless-verified first, then seeders desc — same order the app uses
            results.sort_by(|a, b| {
                let la = a.get("lossless").and_then(|v| v.as_bool()) == Some(true);
                let lb = b.get("lossless").and_then(|v| v.as_bool()) == Some(true);
                lb.cmp(&la).then(
                    b.get("seeds")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                        .cmp(&a.get("seeds").and_then(|v| v.as_u64()).unwrap_or(0)),
                )
            });
            let issues: Vec<String> = resp
                .get("provider_errors")
                .and_then(|e| e.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|i| {
                            i.get("error")
                                .or_else(|| i.get("message"))
                                .and_then(|m| m.as_str())
                                .map(String::from)
                        })
                        .collect()
                })
                .unwrap_or_default();
            s.status
                .set_label(&if results.is_empty() && issues.is_empty() {
                    "No results — try a broader query".to_string()
                } else {
                    issues.join(" · ")
                });
            s.results = results;
            s.detail.clear();
            s.expanded = None;
        }
        render_rows(app, &st);
    });
}

pub fn on_resolved(app: &Shared, idx: usize, v: Value) {
    DISC.with(|d| {
        let Some(st) = d.borrow().as_ref().cloned() else {
            return;
        };
        st.borrow_mut().detail.insert(idx, v);
        render_rows(app, &st);
    });
}

fn render_rows(app: &Shared, st: &Rc<RefCell<Disc>>) {
    let s = st.borrow();
    while let Some(r) = s.list.first_child() {
        s.list.remove(&r);
    }
    for (i, r) in s.results.iter().enumerate() {
        let row_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        row_box.set_margin_top(6);
        row_box.set_margin_bottom(6);
        let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let provider = r.get("provider").and_then(|v| v.as_str()).unwrap_or("?");
        let seeds = r.get("seeds").and_then(|v| v.as_u64());
        let size = r.get("size_bytes").and_then(|v| v.as_u64());
        let lossless = r.get("lossless").and_then(|v| v.as_bool());
        let title = gtk4::Label::new(Some(name));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title.set_hexpand(true);
        line.append(&title);
        if lossless == Some(true) {
            let badge = gtk4::Label::new(Some("lossless"));
            badge.add_css_class("mint");
            line.append(&badge);
        }
        let meta = format!(
            "{}{}{}",
            provider,
            seeds.map(|s| format!(" · {s} seeds")).unwrap_or_default(),
            size.map(|b| format!(" · {:.0} MiB", b as f64 / 1_048_576.0))
                .unwrap_or_default()
        );
        let meta_l = gtk4::Label::new(Some(&meta));
        meta_l.add_css_class("dim");
        line.append(&meta_l);
        row_box.append(&line);

        // expanded detail: file preview + add button
        if s.expanded == Some(i) {
            let det = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
            det.add_css_class("lyra-card");
            match s.detail.get(&i) {
                None => {
                    let l = gtk4::Label::new(Some("resolving files…"));
                    l.add_css_class("dim");
                    det.append(&l);
                }
                Some(d) => {
                    if let Some(files) = d.get("files").and_then(|f| f.as_array()) {
                        for f in files.iter().take(12) {
                            let n = f.get("path").and_then(|p| p.as_str()).unwrap_or("?");
                            let sz = f.get("size").and_then(|p| p.as_u64()).unwrap_or(0);
                            let l = gtk4::Label::new(Some(&format!(
                                "{n}  ·  {:.1} MiB",
                                sz as f64 / 1_048_576.0
                            )));
                            l.set_xalign(0.0);
                            l.add_css_class("dim");
                            det.append(&l);
                        }
                    }
                    let addable = d.get("addable").cloned().unwrap_or(Value::Null);
                    if !addable.is_null() {
                        let add = gtk4::Button::with_label("Add to downloads");
                        add.add_css_class("suggested-action");
                        add.set_halign(gtk4::Align::Start);
                        let app = app.clone();
                        add.connect_clicked(move |_| {
                            // addable = {kind: magnet|torrent_url|torrent_b64, …}
                            let spec = addable
                                .get("magnet")
                                .or_else(|| addable.get("torrent_url"))
                                .or_else(|| addable.get("url"))
                                .and_then(|v| v.as_str())
                                .map(String::from)
                                .or_else(|| {
                                    addable
                                        .get("torrent_b64")
                                        .and_then(|v| v.as_str())
                                        .map(|b| format!("base64:{b}"))
                                });
                            if let Some(spec) = spec {
                                let tx = app.tx.clone();
                                std::thread::spawn(move || {
                                    let id = ffi::torrent_add(&spec);
                                    let _ = tx.send(Msg::TorrentAdded(id));
                                });
                            }
                        });
                        det.append(&add);
                    }
                }
            }
            row_box.append(&det);
        }
        s.list.append(&row_box);
    }
}

thread_local! {
    static DISC: RefCell<Option<Rc<RefCell<Disc>>>> = const { RefCell::new(None) };
}
