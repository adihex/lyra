# Test-isolation policy (lyra-store + lyra-torrent)

Tests share nothing: no fixed paths, no shared database, no global
mutable fixture. A test that only passes when run alone is a bug in
the test, and the rules below exist to make that class of bug hard
to write.

## Rules

1. **One database per test.** `Library::open_memory()` for pure
   relational tests; file-backed `Library::open()` inside a unique
   temp dir for anything touching the artwork cache. See
   `crates/lyra-store/tests/isolation.rs`.
2. **Unique temp dirs, always.** Never `temp_dir().join("fixed-name")`
   (a previous crashed run's leftovers leak into the next) and never a
   bare pid suffix (two tests in one process share a pid). Use the
   `unique_temp_dir(prefix)` helper (`tests/common/mod.rs`: pid +
   timestamp + atomic sequence), and remove the dir at test end.
3. **One session dir per torrent test.** Never reuse the e2e's fixed
   `lyra-torrent-e2e` path outside `live.rs`. See
   `crates/lyra-torrent/tests/isolation.rs`.
4. **Fixed ports are forbidden in new tests.** The legacy e2e binds
   `16100..16102`; new session tests use `listen_port_range: None`
   (OS-assigned) with DHT disabled, so parallel engines never
   contend. If you must bind a fixed port, the test is serial by
   construction — say so in a comment.
5. **Skip, don't fake.** Missing `ffmpeg` / missing `~/Music/Lyra-Test`
   fixture → early return with a notice (existing pattern). A test
   that can't set up must not pretend it ran.

## Threading (`RUST_TEST_THREADS`)

The default is full parallelism, and the suite must stay green that
way — `memory_libraries_are_thread_isolated` and
`engines_are_thread_isolated` prove it on every run. When triaging a
suspected flake, re-run with `RUST_TEST_THREADS=1`:

```sh
RUST_TEST_THREADS=1 cargo test -p lyra-store -p lyra-torrent
```

If the failure disappears under serial execution, it's contention
for a shared resource (port, path, device) — fix the sharing, don't
cap the threads. (libtest reads `RUST_TEST_THREADS` from the
environment; there is no Cargo config key for it, so it is
documented here and in `.cargo/config.toml`, not defaulted.)

## Configuration

`.cargo/config.toml` sets `RUST_BACKTRACE=1` for every test process
so panics in CI logs (especially flake-sweep logs) carry backtraces.
It deliberately does *not* set `RUST_TEST_THREADS` — see above.
