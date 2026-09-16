//! lyra-mapgen: headless batch map generator — same Rust crates as the
//! in-app pipeline, runnable on a beefy host. Reads local files (remote
//! `ByteSource`s arrive `CachingSource`-wrapped through the same
//! `MapGen::for_track` seam) and drops `.lyramap` artifacts + registry rows.
//!
//! Usage: lyra-mapgen <input>… [-o out.lyramap] [--models-dir DIR]
//!        [--stages grid,chords,…|all] [--maps-dir DIR] [--db library.db]

use lyra_map::{encode_map, record_for, MapGen, MapOptions, Stage, StageSet};
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "usage: lyra-mapgen <input>… [-o out.lyramap] [--models-dir DIR]\n\
         \x20       [--stages grid,sections,chords,notes,tab|all] [--maps-dir DIR] [--db library.db]"
    );
    std::process::exit(2);
}

struct Args {
    inputs: Vec<PathBuf>,
    out: Option<PathBuf>,
    models_dir: Option<PathBuf>,
    stages: StageSet,
    maps_dir: Option<PathBuf>,
    db: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut a = Args {
        inputs: Vec::new(),
        out: None,
        models_dir: None,
        stages: StageSet::default(),
        maps_dir: None,
        db: None,
    };
    let mut it = std::env::args().skip(1).peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-o" => a.out = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--models-dir" => {
                a.models_dir = Some(PathBuf::from(it.next().unwrap_or_else(|| usage())))
            }
            "--stages" => {
                a.stages = StageSet::parse(&it.next().unwrap_or_else(|| usage()))
            }
            "--maps-dir" => {
                a.maps_dir = Some(PathBuf::from(it.next().unwrap_or_else(|| usage())))
            }
            "--db" => a.db = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            s if s.starts_with('-') => usage(),
            s => a.inputs.push(PathBuf::from(s)),
        }
    }
    if a.inputs.is_empty() {
        usage();
    }
    if a.out.is_some() && a.inputs.len() > 1 {
        eprintln!("lyra-mapgen: -o takes one input; writing per-input maps instead");
        a.out = None;
    }
    a
}

fn main() {
    let args = parse_args();
    // Env-gated weights: --models-dir wins, else LYRA_TEST_MODELS/LYRA_MODELS_DIR.
    let models_dir = args.models_dir.or_else(lyra_map::grid_models_dir);
    let gen = MapGen::new(MapOptions {
        models_dir,
        stages: args.stages,
        maps_dir: args.maps_dir.clone(),
        ..MapOptions::default()
    })
    .on_progress(|s: Stage, f: f32| {
        eprintln!("  …{:<9} {:>3.0}%", s.as_str(), f * 100.0);
    });

    let mut failed = 0;
    for input in &args.inputs {
        eprintln!("{}:", input.display());
        match gen.for_path(input) {
            Ok(mut map) => {
                let bytes = match encode_map(&mut map) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("  encode failed: {e}");
                        failed += 1;
                        continue;
                    }
                };
                let out = match &args.out {
                    Some(o) => o.clone(),
                    None => {
                        let dir = args
                            .maps_dir
                            .clone()
                            .unwrap_or_else(|| PathBuf::from("maps"));
                        if let Err(e) = std::fs::create_dir_all(&dir) {
                            eprintln!("  maps dir failed: {e}");
                            failed += 1;
                            continue;
                        }
                        dir.join(format!("{}.lyramap", map.audio_hash))
                    }
                };
                if let Err(e) = std::fs::write(&out, &bytes) {
                    eprintln!("  write failed: {e}");
                    failed += 1;
                    continue;
                }
                let rec = record_for(&map, &out.display().to_string());
                if let Some(db) = &args.db {
                    match lyra_store::Library::open(db) {
                        Ok(lib) => {
                            let row = lyra_store::TrackMapRow {
                                audio_hash: rec.audio_hash.clone(),
                                map_path: rec.map_path.clone(),
                                pipeline_ver: rec.pipeline_ver.clone(),
                                status: rec.status.clone(),
                                overall_conf: rec.overall_conf,
                                updated_at: rec.updated_at,
                            };
                            if let Err(e) = lib.upsert_map(&row)
                                .and_then(|_| {
                                    lib.set_track_audio_hash(
                                        &input.display().to_string(),
                                        &rec.audio_hash,
                                    )
                                })
                            {
                                eprintln!("  registry failed: {e}");
                            }
                        }
                        Err(e) => eprintln!("  db open failed: {e}"),
                    }
                }
                println!(
                    "{} -> {} [status={} conf={:.2} beats={} chords={} notes={} tab={}]",
                    input.display(),
                    out.display(),
                    rec.status,
                    map.quality.overall,
                    map.grid.beats.len(),
                    map.chords.len(),
                    map.notes.len(),
                    map.tab.len(),
                );
            }
            Err(e) => {
                eprintln!("  failed: {e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
}
