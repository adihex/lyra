//! Torrent integration tests.
//!
//! `local_seed_leech_decode` — self-contained e2e: a local seeder session
//! serves a real FLAC; the leecher adds it BY MAGNET (metadata resolved
//! from the peer), streams via TorrentFileSource, and decodes through
//! TrackDecoder. No external network. Run:
//!   cargo test -p lyra-torrent -- --nocapture
//!
//! `archive_org_metadata` — real-network check against an archive.org
//! etree item. Metadata-only on purpose: archive.org torrents carry a
//! BEP19 `url-list` web seed and a long-dead tracker — rqbit doesn't
//! implement web seeds, so swarm reads stall. HTTP range fallback for
//! url-list entries is a blueprint TODO.

use lyra_fs::{ByteSource, SourceMediaSource};
use lyra_torrent::{AddOpts, EngineConfig, TorrentEngine};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wait_finished(engine: &TorrentEngine, id: usize, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if engine
            .stats(id)
            .ok()
            .and_then(|s| s["finished"].as_bool())
            .unwrap_or(false)
        {
            return;
        }
        assert!(Instant::now() < deadline, "torrent never finished/verified");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn local_seed_leech_decode() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("ffmpeg not installed — skipping e2e");
        return;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        seed_leech_body();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(120))
        .expect("e2e timed out");
}

fn seed_leech_body() {
    let tmp = std::env::temp_dir().join("lyra-torrent-e2e");
    let _ = std::fs::remove_dir_all(&tmp);
    let seed_dir = tmp.join("seed");
    let flac = seed_dir.join("e2e-tone.flac");
    std::fs::create_dir_all(&seed_dir).unwrap();

    // A real FLAC: 3s stereo 44.1kHz tone.
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=997:duration=3:sample_rate=44100",
            "-ac",
            "2",
            "-y",
        ])
        .arg(&flac)
        .status()
        .unwrap();
    assert!(ok.success(), "ffmpeg flac generation failed");

    // Build a single-file torrent for the FLAC.
    let seeder = TorrentEngine::new_with_config(
        tmp.join("seeder-session"),
        EngineConfig {
            disable_dht: true,
            listen_port_range: Some(16100..16102),
        },
    )
    .unwrap();
    let torrent_bytes = seeder.create_torrent_bytes(&flac).unwrap();
    let torrent_path = tmp.join("e2e.torrent");
    std::fs::write(&torrent_path, &torrent_bytes).unwrap();

    // Seeder: same file already on disk → verify, then serve.
    let seed_id = seeder
        .add_opts(
            torrent_path.to_str().unwrap(),
            AddOpts {
                overwrite: true,
                output_folder: Some(seed_dir.clone()),
                disable_trackers: true,
                ..Default::default()
            },
        )
        .unwrap();
    wait_finished(&seeder, seed_id, 15);
    let port = seeder.listen_port().expect("seeder not listening");
    eprintln!("seeder live on 127.0.0.1:{port}");

    // Leecher: add BY MAGNET — metadata must come from the seeder peer.
    let info: librqbit::TorrentMetaV1Borrowed =
        librqbit::torrent_from_bytes(&torrent_bytes).unwrap();
    let magnet = librqbit::Magnet::from_id20(info.info_hash, Vec::new(), None).to_string();
    let leecher = Arc::new(
        TorrentEngine::new_with_config(
            tmp.join("leech"),
            EngineConfig {
                disable_dht: true,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let id = leecher
        .add_opts(
            &magnet,
            AddOpts {
                initial_peers: Some(vec![SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)]),
                disable_trackers: true,
                ..Default::default()
            },
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    let files = loop {
        match leecher.files(id) {
            Ok(f) if !f.is_empty() => break f,
            _ if Instant::now() > deadline => {
                panic!("magnet metadata never resolved from peer")
            }
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    };
    let f = files
        .iter()
        .find(|f| f.path.ends_with(".flac"))
        .unwrap()
        .clone();
    eprintln!("leecher sees: {} ({} bytes)", f.path, f.len);

    // Stream-while-downloading: head read pulls the first pieces.
    let src: Arc<dyn ByteSource> = leecher.open_file(id, f.index).unwrap();
    let mut head = [0u8; 64];
    let n = src.read_at(0, &mut head).unwrap();
    assert!(
        n >= 4 && &head[..4] == b"fLaC",
        "bad head magic: {:?}",
        &head[..4]
    );
    eprintln!("head read ok — fLaC magic streamed over loopback swarm");

    // Header probe — the path lyra_torrent_probe uses for UI durations.
    let probe_src: Arc<dyn ByteSource> = leecher.open_file(id, f.index).unwrap();
    let probe_media = SourceMediaSource::new(lyra_fs::CachingSource::wrap(probe_src));
    let info =
        lyra_formats::stream_info_media(probe_media, lyra_formats::format_from_ext("flac"), "flac")
            .unwrap();
    assert!(
        info.duration_secs.unwrap_or(0.0) > 0.0,
        "probe over torrent stream returned no duration"
    );
    eprintln!(
        "probe: {:.1}s {} {}Hz",
        info.duration_secs.unwrap_or(0.0),
        info.codec,
        info.sample_rate.unwrap_or(0)
    );

    // The full product path: ByteSource → MediaSource → TrackDecoder.
    let media = SourceMediaSource::new(src);
    let mut dec = lyra_formats::TrackDecoder::open(media, Some("flac")).unwrap();
    assert_eq!(dec.sample_rate, 44100);
    assert_eq!(dec.channels, 2);
    let mut frames = 0usize;
    while let Some(block) = dec.next_block().unwrap() {
        frames += block.len() / 2;
        if frames > 44_100 {
            break; // ~1s of audio is proof enough
        }
    }
    assert!(frames > 10_000, "decoded only {frames} frames");
    eprintln!("decoded {frames} frames of streamed torrent audio — PASS");
}

/// Audible variant: leech the FLAC and push it through the real output
/// device — you should HEAR ~2s of tone sourced entirely via BitTorrent.
/// `cargo test -p lyra-torrent -- --ignored audible --nocapture`
#[test]
#[ignore]
fn local_seed_leech_audible() {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        seed_leech_body();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(120))
        .expect("e2e timed out");

    // Re-leech is unnecessary — the file completed during the body run.
    // Play the completed download through the real device.
    let completed = std::env::temp_dir().join("lyra-torrent-e2e/leech/e2e-tone.flac");
    assert!(completed.exists(), "leeched flac missing: {completed:?}");

    let engine = lyra_engine::Engine::new().unwrap();
    engine.play(
        Arc::new(lyra_fs::LocalFile::open(&completed).unwrap()),
        Some("flac"),
    );
    engine.set_volume(0.3);
    std::thread::sleep(Duration::from_millis(2200));
    let pos = engine.position_secs();
    assert!(pos > 0.5, "audible playback stalled at {pos}s");
    eprintln!("played {pos:.2}s of torrent-sourced audio through device");
}

#[test]
#[ignore]
fn archive_org_metadata() {
    let dir = std::env::temp_dir().join("lyra-torrent-archive");
    let _ = std::fs::remove_dir_all(&dir);
    let engine = TorrentEngine::new(dir).unwrap();

    // O.A.R. 2006-01-14 — freely-tradeable etree recording (FLAC16).
    let url = "https://archive.org/download/oar2006-01-14.mix.flac16/oar2006-01-14.mix.flac16-flac.torrent";
    let path = std::env::temp_dir().join("oar2006.torrent");
    if !path.exists() {
        let st = std::process::Command::new("curl")
            .args(["-fsSL", url, "-o"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(st.success(), "torrent fetch failed");
    }

    let id = engine.add(path.to_str().unwrap()).unwrap();
    let files = engine.files(id).unwrap();
    assert_eq!(files.len(), 21);
    assert!(files.iter().all(|f| f.path.ends_with(".flac")));
    eprintln!("archive.org torrent: {} flac files listed", files.len());
}
