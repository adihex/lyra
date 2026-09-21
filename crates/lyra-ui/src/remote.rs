//! Remote pane + the remote-sources card embedded in Library.
//!
//! Two surfaces the app merges under "Remote": LAN device pairing
//! (SPAKE2 → pinned keys → Noise, `lyra_remote_*`) and SSH/SFTP library
//! roots (`lyra_remlib_*` + `lyra_engine_play_remote`). Profiles persist
//! as JSON in the data dir — there's no Keychain on Linux, so passwords
//! stay session-only.

use crate::model::{save_remotes, RemoteSource};
use crate::{engine, ffi, Msg, Shared};
use gtk4::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use std::thread;

struct RemotePane {
    devices: gtk4::ListBox,
    pair_info: gtk4::Label,
    status: gtk4::Label,
    port: gtk4::SpinButton,
    server_on: bool,
}

thread_local! {
    static RP: RefCell<Option<Rc<RefCell<RemotePane>>>> = const { RefCell::new(None) };
    /// Test/scan results index into this — set when the card is built.
    static REMOTES: RefCell<Vec<RemoteSource>> = const { RefCell::new(Vec::new()) };
    /// Session passwords — never persisted (no Keychain on Linux).
    static PASSWORDS: RefCell<std::collections::HashMap<String, String>> =
        RefCell::new(std::collections::HashMap::new());
}

pub fn build(app: &Shared) -> gtk4::Widget {
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(20);
    root.set_margin_end(20);

    let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let t = gtk4::Label::new(Some("Remote"));
    t.add_css_class("title-1");
    head.append(&t);
    root.append(&head);

    let desc = gtk4::Label::new(Some(
        "Pair a phone to play your library on it over the LAN — SPAKE2 code, pinned keys, Noise transport.",
    ));
    desc.set_xalign(0.0);
    desc.set_wrap(true);
    desc.add_css_class("dim");
    root.append(&desc);

    // server controls
    let srv = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    srv.add_css_class("lyra-card");
    srv.append(&gtk4::Label::new(Some("Port")));
    let port = gtk4::SpinButton::with_range(1024.0, 65535.0, 1.0);
    port.set_value(app.prefs.borrow().remote_port as f64);
    srv.append(&port);
    let server_btn = gtk4::Button::with_label("Start server");
    server_btn.add_css_class("suggested-action");
    srv.append(&server_btn);
    let status = gtk4::Label::new(None);
    status.add_css_class("dim");
    srv.append(&status);
    root.append(&srv);

    // pairing
    let pair = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    pair.add_css_class("lyra-card");
    let pair_btn = gtk4::Button::with_label("Show pairing code");
    pair_btn.add_css_class("sharp");
    pair.append(&pair_btn);
    let pair_info = gtk4::Label::new(None);
    pair_info.set_selectable(true);
    pair_info.add_css_class("mint");
    pair.append(&pair_info);
    root.append(&pair);

    // paired devices
    let dev_head = gtk4::Label::new(Some("Paired devices"));
    dev_head.set_xalign(0.0);
    dev_head.add_css_class("heading");
    root.append(&dev_head);
    let devices = gtk4::ListBox::new();
    devices.set_selection_mode(gtk4::SelectionMode::None);
    let dscroll = gtk4::ScrolledWindow::builder()
        .child(&devices)
        .vexpand(true)
        .build();
    root.append(&dscroll);

    let rp = Rc::new(RefCell::new(RemotePane {
        devices: devices.clone(),
        pair_info: pair_info.clone(),
        status: status.clone(),
        port: port.clone(),
        server_on: false,
    }));
    RP.with(|r| *r.borrow_mut() = Some(rp.clone()));

    server_btn.connect_clicked({
        let app = app.clone();
        let rp = rp.clone();
        move |b| {
            let mut r = rp.borrow_mut();
            if r.server_on {
                return; // no stop FFI — remote stays up for the session
            }
            let p = r.port.value() as u16;
            let e = engine();
            if e.is_null() {
                r.status
                    .set_label("needs an output device (engine) — none here");
                return;
            }
            let key = app.host.borrow().data_dir.join("remote-key.bin");
            if ffi::remote_init(e, &key) && ffi::remote_start(p) {
                r.server_on = true;
                b.set_label("Serving");
                b.set_sensitive(false);
                r.status.set_label(&format!("remote on :{p}"));
                let mut pr = app.prefs.borrow_mut();
                pr.remote_port = p;
                let dir = app.host.borrow().data_dir.clone();
                pr.save(&dir);
            } else {
                r.status.set_label("remote init/start failed");
            }
        }
    });

    pair_btn.connect_clicked({
        let rp = rp.clone();
        move |_| {
            let v = ffi::remote_open_pairing();
            let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("?");
            let fp = v.get("fp").and_then(|c| c.as_str()).unwrap_or("?");
            rp.borrow()
                .pair_info
                .set_label(&format!("code {code} · fingerprint {fp}"));
        }
    });

    refresh_devices(&rp);
    // poll for new pairings while the pane is visible
    gtk4::glib::timeout_add_local(std::time::Duration::from_secs(3), {
        let rp = rp.clone();
        let stack = app.stack.clone();
        move || {
            if stack.visible_child_name().map(|n| n == "remote") == Some(true) {
                refresh_devices(&rp);
            }
            gtk4::glib::ControlFlow::Continue
        }
    });

    root.upcast()
}

fn refresh_devices(rp: &Rc<RefCell<RemotePane>>) {
    let list = ffi::remote_devices();
    let r = rp.borrow();
    while let Some(c) = r.devices.first_child() {
        r.devices.remove(&c);
    }
    let Some(devs) = list.as_array() else { return };
    for d in devs {
        let id = d
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("device");
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        row.set_margin_top(4);
        row.set_margin_bottom(4);
        let l = gtk4::Label::new(Some(name));
        l.set_xalign(0.0);
        l.set_hexpand(true);
        row.append(&l);
        let revoke = gtk4::Button::with_label("Revoke");
        revoke.add_css_class("sharp");
        {
            let rp = rp.clone();
            let id = id.clone();
            revoke.connect_clicked(move |_| {
                ffi::remote_revoke(&id);
                refresh_devices(&rp);
            });
        }
        row.append(&revoke);
        r.devices.append(&row);
    }
}

// ── remote sources card (lives inside Library, wired from library.rs) ───

/// Fill the card with existing sources + the add form — the same rows
/// the app's remoteSourcesCard renders.
pub fn fill_sources_card(app: &Shared, card: &gtk4::Box, remotes: &[RemoteSource]) {
    REMOTES.with(|r| *r.borrow_mut() = remotes.to_vec());
    rebuild_card(app, card);
}

fn rebuild_card(app: &Shared, card: &gtk4::Box) {
    while let Some(c) = card.first_child() {
        card.remove(&c);
    }
    let data_dir = app.host.borrow().data_dir.clone();
    REMOTES.with(|rs| {
        for (i, s) in rs.borrow().iter().enumerate() {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            let label = gtk4::Label::new(Some(&format!(
                "{}  ·  {}{}:{}",
                s.label(),
                if s.user.is_empty() {
                    String::new()
                } else {
                    format!("{}@", s.user)
                },
                s.host,
                s.root_path
            )));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            row.append(&label);
            for (name, what) in [("Test", 0u8), ("Scan", 1u8)] {
                let b = gtk4::Button::with_label(name);
                b.add_css_class("sharp");
                let app = app.clone();
                b.connect_clicked(move |_| {
                    let profile = REMOTES.with(|r| {
                        r.borrow().get(i).map(|s| {
                            let key = format!("{}:{}", s.host, s.root_path);
                            let pw = PASSWORDS.with(|p| p.borrow().get(&key).cloned());
                            s.profile_json(pw.as_deref())
                        })
                    });
                    let Some(profile) = profile else { return };
                    let tx = app.tx.clone();
                    let db = app.host.borrow().db_path.clone();
                    thread::spawn(move || {
                        let v = if what == 0 {
                            ffi::remlib_test(&profile)
                        } else {
                            ffi::remlib_scan(&db, &profile)
                        };
                        let _ = tx.send(if what == 0 {
                            Msg::RemoteTestDone(i, v)
                        } else {
                            Msg::RemoteScanDone(i, v)
                        });
                    });
                });
                row.append(&b);
            }
            let rm = gtk4::Button::with_label("✕");
            rm.add_css_class("flat");
            {
                let app = app.clone();
                let card = card.clone();
                let data_dir = data_dir.clone();
                rm.connect_clicked(move |_| {
                    REMOTES.with(|r| {
                        let mut v = r.borrow_mut();
                        if i < v.len() {
                            v.remove(i);
                        }
                        save_remotes(&data_dir, &v);
                    });
                    rebuild_card(&app, &card);
                });
            }
            row.append(&rm);
            card.append(&row);
        }
    });

    // ── add form ───────────────────────────────────────────────────────
    let form = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    let mkrow = |fields: &[(&str, &gtk4::Entry)]| {
        let r = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        for (ph, e) in fields {
            e.set_placeholder_text(Some(*ph));
            e.set_hexpand(true);
            r.append(*e);
        }
        r
    };
    let name = gtk4::Entry::new();
    let host_e = gtk4::Entry::new();
    let port_e = gtk4::SpinButton::with_range(1.0, 65535.0, 1.0);
    port_e.set_value(22.0);
    let user_e = gtk4::Entry::new();
    let root_e = gtk4::Entry::new();
    let key_e = gtk4::Entry::new();
    let pw_e = gtk4::PasswordEntry::new();
    pw_e.set_placeholder_text(Some("password (session only)"));

    form.append(&mkrow(&[("Name (optional)", &name)]));
    let row2 = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    host_e.set_placeholder_text(Some("Host (alias, tailnet, or IP)"));
    host_e.set_hexpand(true);
    row2.append(&host_e);
    row2.append(&port_e);
    form.append(&row2);
    form.append(&mkrow(&[
        ("User (empty = ssh default)", &user_e),
        ("Remote root, e.g. /mnt/music", &root_e),
    ]));
    let row4 = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    key_e.set_placeholder_text(Some("SSH key path, e.g. ~/.ssh/id_ed25519"));
    key_e.set_hexpand(true);
    row4.append(&key_e);
    row4.append(&pw_e);
    let save = gtk4::Button::with_label("Save source");
    save.add_css_class("suggested-action");
    row4.append(&save);
    form.append(&row4);
    card.append(&form);

    save.connect_clicked({
        let app = app.clone();
        let card = card.clone();
        move |_| {
            let src = RemoteSource {
                name: name.text().to_string(),
                host: host_e.text().to_string(),
                port: port_e.value() as u16,
                user: user_e.text().to_string(),
                root_path: root_e.text().to_string(),
                key_path: key_e.text().to_string(),
            };
            if src.host.is_empty() || src.root_path.is_empty() {
                return;
            }
            let pw = pw_e.text().to_string();
            if !pw.is_empty() {
                PASSWORDS.with(|p| {
                    p.borrow_mut()
                        .insert(format!("{}:{}", src.host, src.root_path), pw);
                });
            }
            let dir = app.host.borrow().data_dir.clone();
            REMOTES.with(|r| {
                r.borrow_mut().push(src);
                save_remotes(&dir, &r.borrow());
            });
            rebuild_card(&app, &card);
        }
    });
}

pub fn on_test(app: &Shared, i: usize, v: Value) {
    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
    let msg = if ok {
        format!(
            "source {i}: ok — {} files in {}ms",
            v.get("files").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("elapsed_ms").and_then(|x| x.as_u64()).unwrap_or(0)
        )
    } else {
        format!(
            "source {i}: {}",
            v.get("error").and_then(|x| x.as_str()).unwrap_or("failed")
        )
    };
    app.status.set_label(&msg);
}

pub fn on_scan(app: &Shared, _i: usize, v: Value) {
    let added = v.get("added").and_then(|x| x.as_i64()).unwrap_or(0);
    app.status
        .set_label(&format!("remote scan: {added} tracks added"));
    app.host.borrow_mut().reload();
    if let Some(f) = app.library_refresh.borrow().as_ref() {
        f();
    }
}

/// Play an `sftp://…` row — the profile for its host prefix comes from
/// the saved sources (password from the session map if given).
pub fn play_remote(app: &Shared, path: &str) {
    let src = REMOTES.with(|r| {
        r.borrow()
            .iter()
            .find(|s| path.starts_with(&s.uri_prefix()))
            .cloned()
    });
    let Some(src) = src else {
        app.status
            .set_label("no remote source profile matches that path");
        return;
    };
    let key = format!("{}:{}", src.host, src.root_path);
    let pw = PASSWORDS.with(|p| p.borrow().get(&key).cloned());
    let profile = src.profile_json(pw.as_deref());
    let e = engine();
    if ffi::play_remote(e, &profile, path) {
        app.host.borrow_mut().current = Some(path.to_string());
        app.status.set_label(&format!("streaming {path}"));
    } else {
        app.status.set_label("remote play failed");
    }
}
