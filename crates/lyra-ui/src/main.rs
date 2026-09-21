//! `lyra-gui` — Lyra's native Linux shell: a GTK4/libadwaita port of the
//! macOS SwiftUI app (`app/Sources/LyraApp`), running on the same
//! `lyra-ffi` surface and the shared `host` runtime — so `lyra`/`lyra-mcp`
//! drive it identically, and the E2E harness can drive a real UI on both
//! OSes.
//!
//! Sidebar sections mirror the app's SidebarItem set (minus Design Lab —
//! that's the macOS design-tooling pane). The Rust core owns all state;
//! widgets render and call FFI.

use gtk4::prelude::*;
use gtk4::{gdk, glib};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

mod coach;
mod discover;
mod eq;
mod ffi;
mod library;
mod map;
mod model;
mod remote;
mod theme;
mod visuals;

use lyra_ffi::host::{self, Host, HostArgs};
use model::Prefs;

/// Channel messages from worker threads to the GTK main loop.
pub enum Msg {
    ScanDone(serde_json::Value),
    ArtDone(u32),
    SearchDone(serde_json::Value),
    Resolved(usize, serde_json::Value),
    RemoteTestDone(usize, serde_json::Value),
    RemoteScanDone(usize, serde_json::Value),
    MapDone(serde_json::Value),
    TorrentAdded(i32),
}

/// Workers post on a std mpsc channel; the main tick drains it
/// (glib's MainContext::channel is deprecated in glib-rs ≥0.18).
pub type Tx = std::sync::mpsc::Sender<Msg>;

pub struct App {
    pub host: RefCell<Host>,
    pub prefs: RefCell<Prefs>,
    pub tx: Tx,
    pub window: adw::ApplicationWindow,
    pub stack: gtk4::Stack,
    pub sidebar: gtk4::ListBox,
    // transport
    pub art: gtk4::Picture,
    /// art path currently loaded — Picture::filename() is GTK 4.8+; track it.
    pub art_path: RefCell<Option<String>>,
    pub title_l: gtk4::Label,
    pub artist_l: gtk4::Label,
    pub play_btn: gtk4::Button,
    pub seek: gtk4::Scale,
    pub pos_l: gtk4::Label,
    pub dur_l: gtk4::Label,
    pub vol: gtk4::Scale,
    pub scrubbing: Cell<bool>,
    // library pane owns its list internals; the shell only needs the
    // refresh entry point.
    pub library_refresh: RefCell<Option<Box<dyn Fn()>>>,
    pub status: gtk4::Label,
}

pub type Shared = Rc<App>;

pub fn engine() -> *mut lyra_engine::Engine {
    lyra_ffi::lyra_engine_current()
}

fn fmt_secs(s: f64) -> String {
    model::fmt_dur(s)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lyra=info".into()),
        )
        .init();

    let args = parse_args();
    let play_file = args.play.clone();
    // Boot before GTK init: the IPC socket exists before the window so the
    // CLI works even if a display can't be opened.
    let host = match Host::boot(&args.into()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("lyra-gui: {e}");
            std::process::exit(1);
        }
    };

    let app = adw::Application::builder()
        .application_id("app.lyra.player")
        .build();
    // activate is Fn (fires once in practice) — the Host moves out on first call.
    let host = RefCell::new(Some(host));
    app.connect_activate(move |a| {
        build_ui(a, host.borrow_mut().take().unwrap(), play_file.clone())
    });
    // our argv is consumed in parse_args; GTK must not see --socket etc.
    app.run_with_args(&Vec::<String>::new());
}

fn parse_args() -> ParsedArgs {
    let mut a = HostArgs {
        data_dir: host::default_data_dir(),
        socket: lyra_ipc::paths::default_socket_path(),
        remote_port: None,
    };
    let mut play = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let Some(v) = it.next() else {
            eprintln!("usage: lyra-gui [--data-dir DIR] [--socket PATH] [--remote-port PORT] [--play FILE]");
            std::process::exit(2);
        };
        match arg.as_str() {
            "--data-dir" => a.data_dir = PathBuf::from(v),
            "--socket" => a.socket = PathBuf::from(v),
            "--remote-port" => a.remote_port = Some(v.parse().unwrap_or(0)),
            "--play" => play = Some(PathBuf::from(v)),
            _ => {
                eprintln!("unknown arg {arg}");
                std::process::exit(2);
            }
        }
    }
    ParsedArgs {
        data_dir: a.data_dir,
        socket: a.socket,
        remote_port: a.remote_port,
        play,
    }
}

pub struct ParsedArgs {
    pub data_dir: PathBuf,
    pub socket: PathBuf,
    pub remote_port: Option<u16>,
    pub play: Option<PathBuf>,
}

impl From<ParsedArgs> for HostArgs {
    fn from(p: ParsedArgs) -> Self {
        HostArgs {
            data_dir: p.data_dir,
            socket: p.socket,
            remote_port: p.remote_port,
        }
    }
}

fn build_ui(application: &adw::Application, host: Host, play: Option<PathBuf>) {
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(theme::CSS);
    gtk4::style_context_add_provider_for_display(
        &gdk::Display::default().expect("display"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // ── transport bar ──────────────────────────────────────────────────
    let art = gtk4::Picture::builder()
        .width_request(56)
        .height_request(56)
        .build();
    art.add_css_class("lyra-card");

    let title_l = gtk4::Label::builder()
        .label("Nothing playing")
        .xalign(0.0)
        .build();
    title_l.add_css_class("heading");
    let artist_l = gtk4::Label::builder().xalign(0.0).build();
    artist_l.add_css_class("dim");

    let prev_btn = gtk4::Button::with_label("⏮");
    prev_btn.add_css_class("flat");
    let play_btn = gtk4::Button::with_label("▶");
    play_btn.add_css_class("flat");
    let next_btn = gtk4::Button::with_label("⏭");
    next_btn.add_css_class("flat");

    let pos_l = gtk4::Label::new(Some("0:00"));
    pos_l.add_css_class("dim");
    let seek = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.1);
    seek.set_hexpand(true);
    seek.add_css_class("seek");
    seek.set_draw_value(false);
    let dur_l = gtk4::Label::new(Some("0:00"));
    dur_l.add_css_class("dim");

    let vol_icon = gtk4::Label::new(Some("🔊"));
    let vol = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.42, 0.05);
    vol.set_size_request(110, -1);
    vol.set_draw_value(false);

    let transport = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    transport.append(&art);
    let text_col = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    text_col.set_valign(gtk4::Align::Center);
    text_col.append(&title_l);
    text_col.append(&artist_l);
    transport.append(&text_col);
    transport.append(&prev_btn);
    transport.append(&play_btn);
    transport.append(&next_btn);
    transport.append(&pos_l);
    transport.append(&seek);
    transport.append(&dur_l);
    transport.append(&vol_icon);
    transport.append(&vol);
    transport.add_css_class("lyra-now-playing");

    // ── sidebar ────────────────────────────────────────────────────────
    let sidebar = gtk4::ListBox::new();
    sidebar.add_css_class("lyra-sidebar");
    sidebar.set_selection_mode(gtk4::SelectionMode::Single);
    const SECTIONS: [(&str, &str); 7] = [
        ("library", "Library"),
        ("discover", "Discover"),
        ("coach", "Coach"),
        ("map", "Map"),
        ("eq", "Equalizer"),
        ("visuals", "Visuals"),
        ("remote", "Remote"),
    ];
    for (name, label) in SECTIONS {
        let row = gtk4::ListBoxRow::new();
        row.set_child(Some(
            &gtk4::Label::builder().label(label).xalign(0.0).build(),
        ));
        row.set_widget_name(name);
        sidebar.append(&row);
    }

    let stack = gtk4::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk4::StackTransitionType::Crossfade);

    let body = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    let side_wrap = gtk4::ScrolledWindow::builder()
        .child(&sidebar)
        .min_content_width(160)
        .max_content_width(220)
        .build();
    body.append(&side_wrap);
    body.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
    body.append(&stack);
    body.set_vexpand(true);

    let status = gtk4::Label::builder().xalign(0.0).build();
    status.add_css_class("dim");

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let header = adw::HeaderBar::new();
    let win_title = gtk4::Label::new(Some("Lyra"));
    header.set_title_widget(Some(&win_title));
    content.append(&header);
    content.append(&body);
    content.append(&status);
    content.append(&transport);

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .default_width(1100)
        .default_height(720)
        .content(&content)
        .build();

    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    let rx = RefCell::new(rx);

    let prefs = Prefs::load(&host.data_dir);
    vol.set_value(prefs.volume);
    let data_dir = host.data_dir.clone();

    let app = Rc::new(App {
        host: RefCell::new(host),
        prefs: RefCell::new(prefs),
        tx,
        window: window.clone(),
        stack: stack.clone(),
        sidebar: sidebar.clone(),
        art: art.clone(),
        art_path: RefCell::new(None),
        title_l: title_l.clone(),
        artist_l: artist_l.clone(),
        play_btn: play_btn.clone(),
        seek: seek.clone(),
        pos_l: pos_l.clone(),
        dur_l: dur_l.clone(),
        vol: vol.clone(),
        scrubbing: Cell::new(false),
        library_refresh: RefCell::new(None),
        status: status.clone(),
    });

    // ── panes ──────────────────────────────────────────────────────────
    stack.add_named(&library::build(&app), Some("library"));
    stack.add_named(&discover::build(&app), Some("discover"));
    stack.add_named(&coach::build(&app), Some("coach"));
    stack.add_named(&map::build(&app), Some("map"));
    stack.add_named(&eq::build(&app), Some("eq"));
    stack.add_named(&visuals::build(&app), Some("visuals"));
    stack.add_named(&remote::build(&app), Some("remote"));

    // ── signals ────────────────────────────────────────────────────────
    sidebar.connect_row_selected({
        let stack = stack.clone();
        move |_, row| {
            if let Some(r) = row {
                stack.set_visible_child_name(&r.widget_name());
            }
        }
    });
    sidebar.select_row(sidebar.row_at_index(0).as_ref());

    {
        let app = app.clone();
        play_btn.connect_clicked(move |_| {
            let e = engine();
            if ffi::is_playing(e) {
                ffi::pause(e);
            } else if ffi::can_resume(e) {
                ffi::resume(e);
            } else if let Some(p) = app.host.borrow().current.clone() {
                // finished → replay; else start queue head
                app.host.borrow_mut().play_path(&p);
            } else {
                app.host.borrow_mut().play_index(0);
            }
        });
    }
    {
        let app = app.clone();
        next_btn.connect_clicked(move |_| app.host.borrow_mut().step(1));
    }
    {
        let app = app.clone();
        prev_btn.connect_clicked(move |_| app.host.borrow_mut().step(-1));
    }
    {
        let app = app.clone();
        seek.connect_change_value(move |_, _, v| {
            app.scrubbing.set(true);
            let dur = app
                .host
                .borrow()
                .current_track()
                .and_then(|t| t.duration_secs)
                .unwrap_or(0.0);
            let e = engine();
            ffi::seek(e, v * dur);
            app.scrubbing.set(false);
            glib::Propagation::Proceed
        });
    }
    {
        let app = app.clone();
        let data_dir2 = data_dir.clone();
        vol.connect_value_changed(move |s| {
            let v = s.value();
            ffi::set_volume(engine(), v as f32);
            let mut p = app.prefs.borrow_mut();
            p.volume = v;
            p.save(&data_dir2);
        });
    }
    ffi::set_volume(engine(), app.prefs.borrow().volume as f32);

    // ── the host tick — same 150 ms cadence as lyrad's loop, plus the
    //    worker → main channel drain ────────────────────────────────────
    glib::timeout_add_local(Duration::from_millis(150), {
        let app = app.clone();
        move || {
            for msg in rx.borrow_mut().try_iter() {
                handle_msg(&app, msg);
            }
            tick(&app);
            glib::ControlFlow::Continue
        }
    });

    if let Some(p) = play {
        app.host.borrow_mut().play_path(&p.display().to_string());
    }
    if let Some(f) = app.library_refresh.borrow().as_ref() {
        f();
    }
    window.present();
}

fn handle_msg(app: &Shared, msg: Msg) {
    match msg {
        Msg::ScanDone(stats) => {
            app.status.set_label(&format!(
                "scan: {}",
                stats.get("added").and_then(|v| v.as_i64()).unwrap_or(0)
            ));
            app.host.borrow_mut().reload();
            refresh_library(app);
        }
        Msg::ArtDone(n) => {
            app.status
                .set_label(&format!("artwork: {n} covers applied"));
            refresh_library(app);
            refresh_transport(app, true);
        }
        Msg::SearchDone(resp) => discover::on_results(app, resp),
        Msg::Resolved(idx, v) => discover::on_resolved(app, idx, v),
        Msg::RemoteTestDone(i, v) => remote::on_test(app, i, v),
        Msg::RemoteScanDone(i, v) => remote::on_scan(app, i, v),
        Msg::MapDone(v) => map::on_map(app, v),
        Msg::TorrentAdded(id) => {
            app.status.set_label(&format!("torrent {id} added"));
            refresh_library(app);
        }
    }
}

/// Refresh the library pane's rows — the pane installs this callback.
fn refresh_library(app: &Shared) {
    if let Some(f) = app.library_refresh.borrow().as_ref() {
        f();
    }
}

/// 150 ms host tick: drain IPC ops, publish state, refresh transport.
fn tick(app: &Shared) {
    {
        let mut h = app.host.borrow_mut();
        h.drain_commands();
        h.publish_state_if_changed();
    }
    refresh_transport(app, false);
    coach::poll(app);
}

fn refresh_transport(app: &Shared, force_art: bool) {
    let e = engine();
    let (playing, pos) = (ffi::is_playing(e), ffi::position(e));
    app.play_btn.set_label(if playing { "⏸" } else { "▶" });

    let cur = app
        .host
        .borrow()
        .current_track()
        .map(model::Track::from_library);
    if let Some(t) = &cur {
        app.title_l.set_label(&t.title);
        app.artist_l
            .set_label(&format!("{} — {}", t.artist, t.album));
        let dur = t.duration;
        app.dur_l.set_label(&fmt_secs(dur));
        if !app.scrubbing.get() {
            app.seek.set_value(if dur > 0.0 { pos / dur } else { 0.0 });
        }
        let want = t
            .artwork_hash
            .as_ref()
            .map(|h| app.host.borrow().artwork_path(h, 64).display().to_string());
        if force_art || *app.art_path.borrow() != want {
            let p = t
                .artwork_hash
                .as_ref()
                .map(|h| app.host.borrow().artwork_path(h, 64));
            match p.filter(|p| p.exists()) {
                Some(p) => {
                    app.art.set_filename(Some(&p));
                    *app.art_path.borrow_mut() = want.clone();
                }
                None => {
                    app.art.set_filename(None::<&PathBuf>);
                    *app.art_path.borrow_mut() = None;
                }
            }
        }
    } else {
        app.art.set_filename(None::<&PathBuf>);
        *app.art_path.borrow_mut() = None;
        app.title_l.set_label("Nothing playing");
        app.artist_l.set_label("");
        app.dur_l.set_label("0:00");
        if !app.scrubbing.get() {
            app.seek.set_value(0.0);
        }
    }
    app.pos_l.set_label(&fmt_secs(pos));
}
