//! QA helper: create a single-file torrent for PATH and seed it on
//! 127.0.0.1:PORT until killed. Prints the magnet (with x.pe hint) to
//! stdout — paste that into the app's Add-torrent field.
//!
//!   cargo run -p lyra-torrent --example seed_track -- /path/file.flac 16100

use lyra_torrent::{AddOpts, EngineConfig, TorrentEngine};

fn main() {
    let file = std::env::args().nth(1).expect("usage: seed_track FILE [PORT]");
    let port: u16 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(16100);
    let path = std::path::PathBuf::from(&file);
    assert!(path.exists(), "{file} missing");

    let work = std::env::temp_dir().join("lyra-ui-seed");
    let _ = std::fs::remove_dir_all(&work);
    let seeder = TorrentEngine::new_with_config(
        work.join("session"),
        EngineConfig { disable_dht: true, listen_port_range: Some(port..port + 2) },
    )
    .unwrap();

    let torrent_bytes = seeder.create_torrent_bytes(&path).unwrap();
    let torrent_path = work.join("seed.torrent");
    std::fs::write(&torrent_path, &torrent_bytes).unwrap();

    let id = seeder
        .add_opts(
            torrent_path.to_str().unwrap(),
            AddOpts {
                overwrite: true,
                output_folder: Some(path.parent().unwrap().to_path_buf()),
                disable_trackers: true,
                ..Default::default()
            },
        )
        .unwrap();

    // wait for initial hash-verify
    for _ in 0..75 {
        if seeder
            .stats(id)
            .ok()
            .and_then(|s| s["finished"].as_bool())
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    let info: librqbit::TorrentMetaV1Borrowed =
        librqbit::torrent_from_bytes(&torrent_bytes).unwrap();
    let port = seeder.listen_port().unwrap();
    println!(
        "magnet:?xt=urn:btih:{}&dn={}&x.pe=127.0.0.1:{}",
        info.info_hash.as_string(),
        path.file_name().unwrap().to_string_lossy(),
        port
    );
    println!("torrent file: {}", torrent_path.display());
    eprintln!("seeding on 127.0.0.1:{port} — ctrl-c to stop");

    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
