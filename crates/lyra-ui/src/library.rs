//! Library pane — mirrors the app's `libraryPane`: search field, sortable
//! track table (library rows + torrent-file rows), scan folder, add
//! torrent (magnet/.torrent), remote sources card, artwork fetch,
//! torrents chips + orphan cleanup.

use crate::model::{fmt_dur, load_remotes, RemoteSource, Track};
use crate::{design, ffi, remote, Msg, Shared};
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::thread;

#[derive(Clone, Copy, PartialEq, Debug)]
enum SortKey {
    TrackNo,
    Title,
    Artist,
    Album,
    Time,
    Format,
}

struct Lib {
    query: String,
    sort: SortKey,
    sort_asc: bool,
    all_tracks: Vec<Track>,
    rows: gtk4::ListBox,
    count_l: gtk4::Label,
    header_btns: [(SortKey, gtk4::Button); 6],
    art_btn: gtk4::Button,
    magnet_revealer: gtk4::Revealer,
    magnet_entry: gtk4::Entry,
    torrents_box: gtk4::Box,
    orphans_row: gtk4::Box,
    remote_card: gtk4::Box,
    remotes: Vec<RemoteSource>,
}

pub fn build(app: &Shared) -> gtk4::Widget {
    let root = design::pane_root();

    // ── header row ─────────────────────────────────────────────────────
    let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let title = design::section_title("Library");
    top.append(&title);

    let search = gtk4::Entry::builder()
        .placeholder_text("Filter…")
        .hexpand(false)
        .width_request(240)
        .build();
    top.append(&search);
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    top.append(&spacer);

    let add_torrent = design::secondary_button("Add torrent");
    add_torrent.set_tooltip_text(Some(
        "Paste a magnet URI or pick a .torrent — audio streams on demand",
    ));
    let remote_btn = design::secondary_button("Remote");
    remote_btn.set_tooltip_text(Some(
        "SSH/SFTP library roots — scan and stream without copying",
    ));
    let art_btn = design::secondary_button("Fetch artwork");
    art_btn.set_tooltip_text(Some(
        "Cover Art Archive lookup for albums missing art (MusicBrainz-paced)",
    ));
    let scan_btn = design::primary_button("Scan folder…");
    for b in [&add_torrent, &remote_btn, &art_btn, &scan_btn] {
        top.append(b);
    }
    root.append(&top);

    // ── magnet entry ───────────────────────────────────────────────────
    let magnet = design::card(gtk4::Orientation::Horizontal);
    let magnet_entry = gtk4::Entry::builder()
        .placeholder_text("magnet:?xt=… or /path/to/file.torrent")
        .hexpand(true)
        .build();
    let magnet_add = design::primary_button("Add");
    let magnet_browse = design::secondary_button("Browse…");
    magnet.append(&magnet_entry);
    magnet.append(&magnet_add);
    magnet.append(&magnet_browse);
    let magnet_revealer = gtk4::Revealer::builder()
        .child(&magnet)
        .reveal_child(false)
        .build();
    root.append(&magnet_revealer);

    // ── remote sources card (collapsed into Remote pane? no — app keeps
    //    it inside Library) ─────────────────────────────────────────────
    let remote_card = design::card(gtk4::Orientation::Vertical);
    remote_card.set_visible(false);
    root.append(&remote_card);

    // ── torrent chips + orphans ────────────────────────────────────────
    let torrents_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    root.append(&torrents_box);
    let orphans_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    orphans_row.set_visible(false);
    let orphans_l = design::dim_label("");
    let purge = design::secondary_button("Clean up");
    orphans_row.append(&orphans_l);
    orphans_row.append(&purge);
    root.append(&orphans_row);

    let scan_status = design::dim_label("");
    root.append(&scan_status);

    // ── track table: header row + listbox ──────────────────────────────
    let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    header.add_css_class("track-header");
    let header_btns = [
        (SortKey::TrackNo, "#", 40),
        (SortKey::Title, "Title", 220),
        (SortKey::Artist, "Artist", 160),
        (SortKey::Album, "Album", 160),
        (SortKey::Time, "Time", 56),
        (SortKey::Format, "Format", 64),
    ];
    let mut btns = Vec::new();
    for (k, label, w) in header_btns {
        let b = gtk4::Button::with_label(label);
        b.set_width_request(w);
        header.append(&b);
        btns.push((k, b));
    }
    root.append(&header);
    root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    let rows = gtk4::ListBox::new();
    rows.add_css_class("track");
    rows.set_selection_mode(gtk4::SelectionMode::Multiple);
    rows.set_activate_on_single_click(false);
    let scroll = gtk4::ScrolledWindow::builder()
        .child(&rows)
        .vexpand(true)
        .build();
    root.append(&scroll);

    let bottom = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let count_l = design::dim_label("0 tracks");
    bottom.append(&count_l);
    let spacer2 = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer2.set_hexpand(true);
    bottom.append(&spacer2);
    let play_sel = design::secondary_button("Play selected");
    bottom.append(&play_sel);
    root.append(&bottom);

    let lib = Rc::new(RefCell::new(Lib {
        query: String::new(),
        sort: SortKey::Artist,
        sort_asc: true,
        all_tracks: Vec::new(),
        rows: rows.clone(),
        count_l: count_l.clone(),
        header_btns: btns.clone().try_into().unwrap(),
        art_btn: art_btn.clone(),
        magnet_revealer: magnet_revealer.clone(),
        magnet_entry: magnet_entry.clone(),
        torrents_box: torrents_box.clone(),
        orphans_row: orphans_row.clone(),
        remote_card: remote_card.clone(),
        remotes: Vec::new(),
    }));
    lib.borrow_mut().remotes = load_remotes(&app.host.borrow().data_dir);
    remote::fill_sources_card(app, &remote_card, &lib.borrow().remotes);

    // Install the shell's refresh hook — rebuilds rows from host state.
    {
        let app2 = app.clone();
        let lib2 = lib.clone();
        *app.library_refresh.borrow_mut() = Some(Box::new(move || rebuild(&app2, &lib2)));
    }

    // ── wiring ─────────────────────────────────────────────────────────
    {
        let lib = lib.clone();
        search.connect_changed(move |e| {
            lib.borrow_mut().query = e.text().to_string();
            refresh_rows(&lib);
        });
    }
    for (k, b) in btns.iter() {
        let lib = lib.clone();
        let k = *k;
        b.connect_clicked(move |_| {
            let mut l = lib.borrow_mut();
            if l.sort == k {
                l.sort_asc = !l.sort_asc;
            } else {
                l.sort = k;
                l.sort_asc = true;
            }
            drop(l);
            refresh_rows(&lib);
        });
    }
    {
        let app = app.clone();
        rows.connect_row_activated(move |_, row| {
            let path = row.widget_name().to_string();
            {
                let p = path.as_str();
                if p.starts_with("torrent://") {
                    if let Some((id, idx)) = parse_torrent_uri(p) {
                        ffi::play_torrent(crate::engine(), id, idx);
                        app.host.borrow_mut().current = Some(p.to_string());
                    }
                } else if p.starts_with("sftp://") {
                    remote::play_remote(&app, p);
                } else {
                    app.host.borrow_mut().play_path(p);
                }
            }
        });
    }
    {
        let app = app.clone();
        let lib = lib.clone();
        play_sel.connect_clicked(move |_| {
            let l = lib.borrow();
            if let Some(row) = l.rows.selected_rows().first() {
                let p = row.widget_name();
                drop(l);
                app.host.borrow_mut().play_path(p.as_str());
            }
        });
    }
    {
        let app = app.clone();
        scan_btn.connect_clicked(move |b| {
            b.set_sensitive(false);
            b.set_label("Scanning…");
            let dialog = gtk4::FileChooserDialog::new(
                Some("Scan folder"),
                Some(&app.window),
                gtk4::FileChooserAction::SelectFolder,
                &[
                    ("Cancel", gtk4::ResponseType::Cancel),
                    ("Scan", gtk4::ResponseType::Accept),
                ],
            );
            let app = app.clone();
            let b = b.clone();
            dialog.connect_response(move |d, resp| {
                d.close();
                let b = b.clone();
                if resp == gtk4::ResponseType::Accept {
                    if let Some(f) = d.file().and_then(|f| f.path()) {
                        let db = app.host.borrow().db_path.clone();
                        let tx = app.tx.clone();
                        let status_l = app.status.clone();
                        status_l.set_label(&format!("scanning {}…", f.display()));
                        thread::spawn(move || {
                            let stats = crate::Host::sync_dir_blocking(&db, &f)
                                .map(|s| serde_json::json!({"added": s.probed, "total": s.walked}))
                                .unwrap_or_default();
                            let _ = tx.send(Msg::ScanDone(stats));
                        });
                    }
                }
                b.set_sensitive(true);
                b.set_label("Scan folder…");
            });
            dialog.present();
        });
    }
    {
        let app = app.clone();
        let lib = lib.clone();
        add_torrent.connect_clicked(move |_| {
            let l = lib.borrow();
            l.magnet_revealer
                .set_reveal_child(!l.magnet_revealer.reveals_child());
            drop(l);
            if lib.borrow().magnet_revealer.reveals_child() {
                lib.borrow().magnet_entry.grab_focus();
            }
            let _ = &app;
        });
    }
    {
        let lib = lib.clone();
        remote_btn.connect_clicked(move |_| {
            let c = &lib.borrow().remote_card;
            c.set_visible(!c.is_visible());
        });
    }
    {
        let app = app.clone();
        let lib = lib.clone();
        let do_add = move || {
            let spec = lib.borrow().magnet_entry.text().to_string();
            let spec = spec.trim().to_string();
            if spec.is_empty() {
                return;
            }
            lib.borrow().magnet_entry.set_text("");
            let tx = app.tx.clone();
            thread::spawn(move || {
                let id = ffi::torrent_add(&spec);
                let _ = tx.send(Msg::TorrentAdded(id));
            });
        };
        let da = do_add.clone();
        magnet_add.connect_clicked(move |_| da());
        magnet_entry.connect_activate(move |_| do_add());
    }
    {
        let app = app.clone();
        magnet_browse.connect_clicked(move |_| {
            let dialog = gtk4::FileChooserDialog::new(
                Some("Pick .torrent"),
                Some(&app.window),
                gtk4::FileChooserAction::Open,
                &[
                    ("Cancel", gtk4::ResponseType::Cancel),
                    ("Open", gtk4::ResponseType::Accept),
                ],
            );
            let tx = app.tx.clone();
            dialog.connect_response(move |d, resp| {
                d.close();
                if resp == gtk4::ResponseType::Accept {
                    if let Some(f) = d.file().and_then(|f| f.path()) {
                        let spec = f.display().to_string();
                        let tx = tx.clone();
                        thread::spawn(move || {
                            let id = ffi::torrent_add(&spec);
                            let _ = tx.send(Msg::TorrentAdded(id));
                        });
                    }
                }
            });
            dialog.present();
        });
    }
    {
        let app = app.clone();
        art_btn.connect_clicked(move |b| {
            b.set_sensitive(false);
            b.set_label("Fetching…");
            let b = b.clone();
            let db = app.host.borrow().db_path.clone();
            let paths: Vec<String> = app
                .host
                .borrow()
                .tracks
                .iter()
                .filter(|t| t.artwork_hash.is_none())
                .map(|t| t.path.clone())
                .collect();
            let tx = app.tx.clone();
            let b = b.clone();
            thread::spawn(move || {
                let mut n = 0u32;
                for p in paths.iter().take(400) {
                    let r = ffi::art_fetch(&db, p);
                    if r.get("applied").and_then(|v| v.as_bool()).unwrap_or(false)
                        || r.get("ok").is_some()
                    {
                        n += 1;
                    }
                }
                let _ = tx.send(Msg::ArtDone(n));
                let _ = b;
            });
        });
    }
    {
        let app = app.clone();
        purge.connect_clicked(move |_| {
            let v = ffi::torrent_purge_orphans();
            let n = v.get("removed").and_then(|r| r.as_u64()).unwrap_or(0);
            app.status.set_label(&format!("purged {n} leftover items"));
            refresh_rows_pub(&app);
        });
    }

    root.upcast()
}

fn parse_torrent_uri(uri: &str) -> Option<(i32, i32)> {
    let rest = uri.strip_prefix("torrent://")?;
    let (id, idx) = rest.split_once('/')?;
    Some((id.parse().ok()?, idx.parse().ok()?))
}

/// Rebuild visible rows from the live model (host.tracks + torrent rows).
fn rebuild(app: &Shared, lib: &Rc<RefCell<Lib>>) {
    {
        let l = lib.borrow();
        l.art_btn.set_sensitive(true);
        l.art_btn.set_label("Fetch artwork");
    }
    let mut tracks: Vec<Track> = app
        .host
        .borrow()
        .tracks
        .iter()
        .map(Track::from_library)
        .collect();
    // Torrent files appear as rows keyed torrent://id/idx — the app does
    // the same so stream-while-downloading is reachable from the table.
    if let Some(list) = ffi::torrent_list().as_array() {
        for t in list {
            if let Some(id) = t.get("id").and_then(|v| v.as_i64()) {
                if let Some(files) = ffi::torrent_files(id as i32).as_array() {
                    for f in files {
                        if let Some(tr) = Track::from_torrent(id as i32, f) {
                            if tr.is_audio() {
                                tracks.push(tr);
                            }
                        }
                    }
                }
            }
        }
    }
    lib.borrow_mut().all_tracks = tracks;
    refresh_rows(lib);
    refresh_torrents(app, lib);
    refresh_orphans(app, lib);
}

fn refresh_torrents(app: &Shared, lib: &Rc<RefCell<Lib>>) {
    let l = lib.borrow();
    while let Some(c) = l.torrents_box.first_child() {
        l.torrents_box.remove(&c);
    }
    drop(l);
    if let Some(list) = ffi::torrent_list().as_array() {
        for t in list {
            let id = t.get("id").and_then(|v| v.as_i64()).unwrap_or(-1) as i32;
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let chip = design::card(gtk4::Orientation::Horizontal);
            let n = gtk4::Label::new(Some(name));
            n.add_css_class("mint");
            let x = gtk4::Button::with_label("✕");
            x.add_css_class("flat");
            let app = app.clone();
            x.connect_clicked(move |_| {
                // "Keep files" default — destructive delete needs a confirm
                // dialog; keep-files is the reversible path.
                ffi::torrent_remove(id, false);
                refresh_rows_pub(&app);
            });
            chip.append(&n);
            chip.append(&x);
            lib.borrow().torrents_box.append(&chip);
        }
    }
}

fn refresh_orphans(app: &Shared, lib: &Rc<RefCell<Lib>>) {
    let orphans = ffi::torrent_orphans();
    let count = orphans.as_array().map(|a| a.len()).unwrap_or(0);
    let l = lib.borrow();
    l.orphans_row.set_visible(count > 0);
    if let Some(lbl) = l
        .orphans_row
        .first_child()
        .and_then(|w| w.downcast::<gtk4::Label>().ok())
    {
        let bytes: u64 = orphans
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|o| o.get("bytes").and_then(|b| b.as_u64()))
                    .sum()
            })
            .unwrap_or(0);
        lbl.set_label(&format!("{count} leftover items · {} B", fmt_bytes(bytes)));
    }
    let _ = app;
}

fn fmt_bytes(b: u64) -> String {
    const U: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

fn refresh_rows_pub(app: &Shared) {
    if let Some(f) = app.library_refresh.borrow().as_ref() {
        f();
    }
}

fn refresh_rows(lib: &Rc<RefCell<Lib>>) {
    let l = lib.borrow_mut();
    while let Some(r) = l.rows.first_child() {
        l.rows.remove(&r);
    }
    let mut ts: Vec<Track> = l.all_tracks.clone();
    if !l.query.is_empty() {
        let q = l.query.to_lowercase();
        ts.retain(|t| {
            t.title.to_lowercase().contains(&q)
                || t.artist.to_lowercase().contains(&q)
                || t.album.to_lowercase().contains(&q)
        });
    }
    let asc = l.sort_asc;
    let key = |t: &Track| -> String {
        match l.sort {
            SortKey::TrackNo => format!("{:08}", t.track_no),
            SortKey::Title => t.title.to_lowercase(),
            SortKey::Artist => t.artist.to_lowercase(),
            SortKey::Album => t.album.to_lowercase(),
            SortKey::Time => format!("{:08.1}", t.duration),
            SortKey::Format => t.codec.to_lowercase(),
        }
    };
    ts.sort_by(|a, b| {
        let c = key(a).cmp(&key(b));
        if asc {
            c
        } else {
            c.reverse()
        }
    });
    for (i, (k, b)) in l.header_btns.iter().enumerate() {
        let label = match (l.sort == *k, l.sort_asc) {
            (true, true) => format!("{}▲", HEADER_LABELS[i]),
            (true, false) => format!("{}▼", HEADER_LABELS[i]),
            _ => HEADER_LABELS[i].to_string(),
        };
        b.set_label(&label);
    }
    for t in &ts {
        l.rows.append(&track_row(t));
    }
    l.count_l.set_label(&if ts.is_empty() {
        "nothing here yet — scan a folder and let it rip".to_string()
    } else {
        format!("{} tracks", ts.len())
    });
}

const HEADER_LABELS: [&str; 6] = ["#", "Title", "Artist", "Album", "Time", "Format"];

fn cell(text: &str, w: i32) -> gtk4::Label {
    let l = gtk4::Label::new(Some(text));
    l.set_width_request(w);
    l.set_xalign(0.0);
    l.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    l.set_max_width_chars(1);
    l
}

fn track_row(t: &Track) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_widget_name(&t.id);
    row.set_tooltip_text(Some(&t.path));
    let b = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    let no_s = if t.track_no == 0 {
        String::new()
    } else {
        t.track_no.to_string()
    };
    let no = cell(&no_s, 40);
    no.add_css_class("dim");
    b.append(&no);
    b.append(&cell(&t.title, 220));
    let artist = cell(&t.artist, 160);
    artist.add_css_class("dim");
    b.append(&artist);
    let album = cell(&t.album, 160);
    album.add_css_class("dim");
    b.append(&album);
    let time = cell(&fmt_dur(t.duration), 56);
    time.add_css_class("dim");
    b.append(&time);
    let codec = cell(&t.codec, 64);
    codec.add_css_class("dim");
    b.append(&codec);
    if t.source != crate::model::Source::File {
        let badge = gtk4::Label::new(Some(match t.source {
            crate::model::Source::Torrent { .. } => " ⇄",
            crate::model::Source::Remote => " ⇄ net",
            crate::model::Source::File => "",
        }));
        badge.add_css_class("mint");
        b.append(&badge);
    }
    row.set_child(Some(&b));
    row
}
