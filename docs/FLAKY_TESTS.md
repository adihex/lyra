# Flaky-test policy (lyra-store + lyra-torrent)

No test is retried automatically — in CI or locally. A retry that
turns red green is a flap buried, not a flap fixed. Flakes stay
visible (`flake-sweep` job, weekly + manual) and, when confirmed,
land in the quarantine list below with a tracking link.

## Quarantine list

| Test | First seen flaking | Tracking | Status |
| ---- | ------------------ | -------- | ------ |
| *(none — no test is currently quarantined)* | | | |

To quarantine: mark the test `#[ignore]` with a comment linking the
tracking issue, add the row above, and keep the test compiling (CI
still builds `#[ignore]` tests; `cargo test` just doesn't run them).
Quarantine is a pause button with an owner, not a graveyard — the
tracking issue must either fix and re-enable the test or delete it.

## Known flap-prone (not quarantined)

These pass consistently but depend on conditions outside the
harness; they degrade to *skip*, never to silent pass:

- `lyra-torrent live::local_seed_leech_decode` — needs `ffmpeg` to
  synthesize its FLAC and loopback swarm timing to leech it. Without
  `ffmpeg` it returns early with a skip notice. If it flakes, suspect
  loopback contention (see `docs/TEST_ISOLATION.md`), not the codec.
- `lyra-store tests::sync_dir_is_incremental` — needs the
  `~/Music/Lyra-Test` fixture; returns early without it. Absent
  fixture = skip, not coverage of `sync_dir`.
- `#[ignore]` by design (never run in CI): `local_seed_leech_audible`
  (needs a real output device) and `archive_org_metadata` (needs the
  public internet + archive.org). Run explicitly with `-- --ignored`.

## Repeat-run schedule

`.github/workflows/test-analytics.yml`, job `flake-sweep`: 5
consecutive `cargo test -p lyra-store -p lyra-torrent` runs, every
Monday 06:00 UTC plus on-demand (`workflow_dispatch`). The loop uses
no `|| true` and no per-test retry — the job exits with the failure
count, and the full log is uploaded as `flake-sweep-log`.

## Triage checklist (when a sweep goes red)

1. Read `flake-sweep-log.txt`: which run(s) failed, which test(s)?
2. Reproduce locally: run that binary 5x; try `RUST_TEST_THREADS=1`
   to separate true flakes from parallelism contention.
3. True flake → file an issue, quarantine per above, fix forward.
4. Deterministic failure → it's a real regression, not a flake:
   fix the code, not the test.
