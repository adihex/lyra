//! lyra-engine: the playback pipeline.
//!
//! ```text
//! ByteSource → TrackDecoder ──[decode+DSP worker]──▶ HeapRb ──[RT callback]──▶ device
//!              (any source)    eq → viz tap          (SPSC f32)   memcpy+gain
//! ```
//!
//! Shape per blueprint §audio: decode worker feeds a bounded ring; the
//! output callback does copy+volume only — no alloc, no locks on the RT
//! thread. cpal = compatibility path; the hog-mode/IOProc driver lands
//! behind the same `ring → callback` contract.
//!
//! Works on every ByteSource equally: local file, SSH block-cached stream,
//! torrent-in-flight — the engine can't tell the difference.

use atomic_float::AtomicF32;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use lyra_core::LyraError;
use lyra_dsp::{Biquad, EqBand, ParametricEq, SafetyLimiter};
use lyra_formats::TrackDecoder;
use lyra_fs::ByteSource;
use lyra_viz::{BeatDetect, Levels, Oscilloscope, SpectrumAnalyzer};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapProd, HeapRb};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tracing::{info, warn};

pub use lyra_viz::VizFrame;

pub mod metrics;
pub use metrics::{EngineMetrics, EngineSnapshot};

/// Ring depth: ~1s of stereo f32 at 192k worst case — covers decode jitter
/// and remote-read latency without pre-buffer stalls.
const RING_SAMPLES: usize = 192_000 * 2;
const EQ_BANDS: usize = 10;

pub enum Command {
    Play {
        source: Arc<dyn ByteSource>,
        extension: Option<String>,
    },
    Pause,
    Resume,
    Stop,
    Seek(f64),
    SetBand(usize, BandSpec),
    Shutdown,
}

/// One EQ band's parameters — what the UI sends.
#[derive(Debug, Clone, Copy)]
pub struct BandSpec {
    pub freq_hz: f32,
    pub q: f32,
    pub gain_db: f32,
    /// false = low shelf, true = peaking (extend as lyra-dsp grows shapes).
    pub peaking: bool,
}

/// Shared viz state — worker writes, UI polls (FFI reads it as a snapshot).
/// `frame` is always fully formed: push() refreshes every field so the
/// FFI read path stays lock + memcpy + seq.
pub struct VizTap {
    levels: Levels,
    spectrum: SpectrumAnalyzer,
    osc: Oscilloscope,
    beat: BeatDetect,
    /// Per-channel clip hold timer (~1.5s) — the frame's sticky bits.
    clip_hold: [f32; 2],
    rate: f32,
    frame: VizFrame,
}

/// dBFS → 0..1 display scale (-60..0 dB → 0..1).
fn db01(db: f32) -> f32 {
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

impl VizTap {
    pub fn new(rate: f32) -> Self {
        Self {
            levels: Levels::new(0.85),
            spectrum: SpectrumAnalyzer::new(lyra_viz::SpectrumConfig {
                sample_rate: rate,
                ..lyra_viz::SpectrumConfig::default()
            }),
            // ~20 ms scope window, strided to 256 points
            osc: Oscilloscope::new(256, (rate * 0.02 / 256.0).round().max(1.0) as u32),
            beat: BeatDetect::new(rate),
            clip_hold: [0.0; 2],
            rate,
            frame: VizFrame::default(),
        }
    }

    /// Audio-thread update — no alloc after init: fixed rings, in-place FFT.
    pub fn push(&mut self, pcm: &[f32]) {
        self.levels.push(pcm);
        self.osc.push(pcm);
        self.beat.push(pcm);
        self.spectrum.feed(pcm);

        let (peak_db, rms_db) = self.levels.read();
        self.spectrum.normalized_into(&mut self.frame.bands);
        self.osc
            .copy_into(&mut self.frame.wave_l, &mut self.frame.wave_r);
        for ch in 0..2 {
            self.frame.peak[ch] = db01(peak_db[ch]);
            self.frame.rms[ch] = db01(rms_db[ch]);
        }
        self.frame.bass = self.frame.bands[..4].iter().sum::<f32>() * 0.25;
        self.frame.beat = self.beat.pulse();
        self.frame.level = (self.frame.rms[0] + self.frame.rms[1]) * 0.5;
        let bc = self.levels.last_clip();
        let dt = (pcm.len() / 2) as f32 / self.rate;
        for ch in 0..2 {
            self.clip_hold[ch] = if bc & (1 << ch) != 0 {
                1.5
            } else {
                (self.clip_hold[ch] - dt).max(0.0)
            };
        }
        self.frame.clip =
            (self.clip_hold[0] > 0.0) as u32 | ((self.clip_hold[1] > 0.0) as u32) << 1;
        self.frame.seq += 1;
    }

    /// Latest fully-formed frame (copy is one memcpy at the FFI layer).
    pub fn frame(&self) -> VizFrame {
        self.frame
    }
}

/// Output path selection.
pub enum OutputMode {
    /// cpal default device — works everywhere, shared with other apps.
    Compat,
    /// CoreAudio HAL: hog mode + IOProc, exclusive access. macOS only,
    /// stereo devices only (multichannel downmix not implemented).
    HalExclusive,
}

#[allow(dead_code)]
enum Output {
    Cpal(cpal::Stream),
    #[cfg(target_os = "macos")]
    Hal {
        proc_: lyra_hal::IoProc,
        keeper: thread::JoinHandle<()>,
    },
}

/// The RT-side consumer: ring → volume → position. Shared verbatim by the
/// cpal data callback and the HAL IOProc pull contract.
///
/// `fill` returns frames sourced from the ring, or `usize::MAX` when the
/// engine is idle (intentional silence — HAL must not count it as an
/// underrun).
struct OutputTap {
    cons: ringbuf::HeapCons<f32>,
    volume: Arc<AtomicF32>,
    playing: Arc<AtomicBool>,
    loaded: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
    position_secs: Arc<AtomicF32>,
    flush: Arc<AtomicBool>,
    out_rate: f32,
    out_ch: usize,
    metrics: Arc<EngineMetrics>,
}

impl OutputTap {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        if self.flush.swap(false, Ordering::Relaxed) {
            self.cons.clear();
        }
        let mut got = 0usize;
        if self.playing.load(Ordering::Relaxed) {
            got = self.cons.pop_slice(out);
            if got == 0 {
                // Playing but the ring is dry — silence went out. Counts
                // here (lock-free) rather than in the worker so short
                // decode stalls are visible in metrics.
                self.metrics.inc_underruns();
            }
            // Natural EOF: decoder finished AND ring drained —
            // now go idle (loaded=false so can_resume is false).
            if got == 0 && self.ended.load(Ordering::Relaxed) {
                self.playing.store(false, Ordering::Relaxed);
                self.loaded.store(false, Ordering::Relaxed);
            }
        }
        let idle = !self.playing.load(Ordering::Relaxed) && got == 0;
        for s in &mut out[got..] {
            *s = 0.0; // underrun → silence
        }
        let g = self.volume.load(Ordering::Relaxed);
        if g != 1.0 {
            for s in &mut out[..got] {
                *s *= g;
            }
        }
        let frames = got / self.out_ch.max(1);
        let prev = self.position_secs.load(Ordering::Relaxed);
        self.position_secs
            .store(prev + frames as f32 / self.out_rate, Ordering::Relaxed);
        if idle {
            usize::MAX
        } else {
            frames
        }
    }
}

pub struct Engine {
    cmd: Sender<Command>,
    volume: Arc<AtomicF32>,
    playing: Arc<AtomicBool>,
    loaded: Arc<AtomicBool>,       // decoder open (even while paused)
    _ended: Arc<AtomicBool>,       // decoder EOF'd — ring may still hold audio
    position_secs: Arc<AtomicF32>, // frames consumed / rate
    viz: Arc<Mutex<VizTap>>,
    /// EQ band specs — shared for response-curve queries; the worker owns
    /// the actual filters and updates both on SetBand.
    eq_specs: Arc<Mutex<Vec<Option<BandSpec>>>>,
    /// Rate the filters are built at (source rate — last opened track).
    eq_rate: Arc<AtomicF32>,
    _output: Output,
    _worker: thread::JoinHandle<()>,
    /// Strong ref held only here — the HAL hog-keeper's Weak dies with
    /// the engine, releasing hog on drop/output-mode swap.
    #[allow(dead_code)]
    keeper_alive: Arc<AtomicBool>,
    /// Playback-event counters (see [`metrics`]) — bumped here, in the
    /// worker, and in the RT callback; read via [`Engine::metrics`].
    metrics: Arc<EngineMetrics>,
}

impl Engine {
    /// Bring up output + worker on the compat (cpal) path.
    pub fn new() -> Result<Self, LyraError> {
        Self::with_output(OutputMode::Compat)
    }

    /// Bring up output + worker on the selected output path.
    pub fn with_output(mode: OutputMode) -> Result<Self, LyraError> {
        let rb = HeapRb::<f32>::new(RING_SAMPLES);
        let (prod, cons) = rb.split();

        let volume = Arc::new(AtomicF32::new(1.0));
        let playing = Arc::new(AtomicBool::new(false));
        let loaded = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let position_secs = Arc::new(AtomicF32::new(0.0));
        let flush = Arc::new(AtomicBool::new(false));
        // Keeper-liveness handle: only the Engine holds a strong ref, so
        // the hog-keeper's Weak fails on engine drop/swap → hog released.
        let keeper_alive = Arc::new(AtomicBool::new(true));
        let metrics = Arc::new(EngineMetrics::default());

        let mk_tap = |cons: ringbuf::HeapCons<f32>, out_rate: f32, out_ch: usize| OutputTap {
            cons,
            volume: Arc::clone(&volume),
            playing: Arc::clone(&playing),
            loaded: Arc::clone(&loaded),
            ended: Arc::clone(&ended),
            position_secs: Arc::clone(&position_secs),
            flush: Arc::clone(&flush),
            out_rate,
            out_ch,
            metrics: Arc::clone(&metrics),
        };

        let (output, out_rate, out_ch) = match mode {
            OutputMode::Compat => {
                let host = cpal::default_host();
                let device = host
                    .default_output_device()
                    .ok_or_else(|| LyraError::Audio("no output device".into()))?;
                let supported = device
                    .default_output_config()
                    .map_err(|e| LyraError::Audio(e.to_string()))?;
                let config: cpal::StreamConfig = supported.into();
                let out_ch = config.channels as usize;
                let out_rate = config.sample_rate as f32;

                // ── RT callback: copy + volume only. No alloc, no locks. ──
                let mut tap = mk_tap(cons, out_rate, out_ch);
                let stream = device
                    .build_output_stream(
                        config,
                        move |out: &mut [f32], _| {
                            tap.fill(out);
                        },
                        |e| warn!("output error: {e}"),
                        None,
                    )
                    .map_err(|e| LyraError::Audio(e.to_string()))?;
                stream.play().map_err(|e| LyraError::Audio(e.to_string()))?;
                (Output::Cpal(stream), out_rate, out_ch)
            }
            OutputMode::HalExclusive => {
                #[cfg(target_os = "macos")]
                {
                    let dev = lyra_hal::HalDevice::default_output()?;
                    let (ch, rate, _il) = dev.virtual_format()?;
                    if ch != 2 {
                        return Err(LyraError::Audio(format!(
                            "HAL path is stereo-only; device has {ch}ch (use Compat)"
                        )));
                    }
                    let mut tap = mk_tap(cons, rate as f32, ch);
                    let proc_ = dev.start_ioproc(Box::new(move |buf| tap.fill(buf)))?;
                    // Hog follows `playing`, not the engine lifetime:
                    // acquired on first audio (~150 ms edge latency),
                    // released on stop/pause/EOF. Held at idle it denies
                    // every other app the device; the IOProc passthrough
                    // keeps the hardware path open while we don't hog.
                    // Hog acquisition can be denied (another hogger) —
                    // retry each tick, audio still flows unhogged.
                    let keeper = {
                        let playing = Arc::clone(&playing);
                        let alive = Arc::downgrade(&keeper_alive);
                        thread::spawn(move || {
                            let mut hog = None;
                            while alive.upgrade().is_some() {
                                match (playing.load(Ordering::Relaxed), hog.is_some()) {
                                    (true, false) => {
                                        hog = dev
                                            .hog()
                                            .map_err(|e| {
                                                warn!("hog denied: {e}");
                                                e
                                            })
                                            .ok();
                                    }
                                    (false, true) => hog = None,
                                    _ => {}
                                }
                                thread::sleep(Duration::from_millis(150));
                            }
                        })
                    };
                    (Output::Hal { proc_, keeper }, rate as f32, ch)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = (cons, &mk_tap);
                    return Err(LyraError::Audio("HAL output is macOS-only".into()));
                }
            }
        };

        let viz = Arc::new(Mutex::new(VizTap::new(out_rate)));
        let eq_specs: Arc<Mutex<Vec<Option<BandSpec>>>> =
            Arc::new(Mutex::new(vec![None; EQ_BANDS]));
        let eq_rate = Arc::new(AtomicF32::new(44_100.0));

        // ── Decode+DSP worker ────────────────────────────────────────────
        let (cmd_tx, cmd_rx) = channel::<Command>();
        let worker = {
            let playing = Arc::clone(&playing);
            let loaded = Arc::clone(&loaded);
            let ended = Arc::clone(&ended);
            let position = Arc::clone(&position_secs);
            let viz = Arc::clone(&viz);
            let flush = Arc::clone(&flush);
            let eq_specs = Arc::clone(&eq_specs);
            let eq_rate = Arc::clone(&eq_rate);
            let metrics = Arc::clone(&metrics);
            thread::spawn(move || {
                worker_loop(WorkerState {
                    cmd: cmd_rx,
                    prod,
                    playing,
                    loaded,
                    ended,
                    position,
                    flush,
                    viz,
                    eq_specs,
                    eq_rate,
                    device_rate: out_rate,
                    device_ch: out_ch,
                    metrics: Arc::clone(&metrics),
                })
            })
        };

        Ok(Self {
            cmd: cmd_tx,
            volume,
            playing,
            loaded,
            _ended: ended,
            position_secs,
            viz,
            eq_specs,
            eq_rate,
            _output: output,
            _worker: worker,
            keeper_alive,
            metrics,
        })
    }

    /// Current in-process metrics snapshot (playback requests, decode
    /// outcomes, underruns). No scrape endpoint on a desktop app — read
    /// this from debug tooling or [`Engine::log_metrics`].
    #[must_use]
    pub fn metrics(&self) -> EngineSnapshot {
        self.metrics.snapshot()
    }

    /// [`Engine::metrics`] rendered as Prometheus exposition-style text.
    #[must_use]
    pub fn metrics_text(&self) -> String {
        self.metrics.snapshot().render_text()
    }

    /// Emit the current snapshot as structured JSON in one log event.
    pub fn log_metrics(&self) {
        let s = self.metrics.snapshot();
        info!(metrics = s.to_json(), "engine metrics");
    }

    /// Play any byte source — local, SSH-cached, torrent, future backends.
    pub fn play(&self, source: Arc<dyn ByteSource>, extension: Option<&str>) {
        self.metrics.inc_play();
        let _ = self.cmd.send(Command::Play {
            source,
            extension: extension.map(String::from),
        });
    }

    pub fn pause(&self) {
        self.metrics.inc_pause();
        let _ = self.cmd.send(Command::Pause);
    }
    pub fn resume(&self) {
        self.metrics.inc_resume();
        let _ = self.cmd.send(Command::Resume);
    }
    pub fn stop(&self) {
        self.metrics.inc_stop();
        let _ = self.cmd.send(Command::Stop);
    }
    pub fn seek(&self, secs: f64) {
        self.metrics.inc_seek();
        let _ = self.cmd.send(Command::Seek(secs));
    }
    pub fn set_band(&self, band: usize, spec: BandSpec) {
        self.metrics.inc_set_band();
        let _ = self.cmd.send(Command::SetBand(band, spec));
    }

    /// Total EQ response at `freqs` (dB) — sums each enabled band's true
    /// biquad response at the rate the filters were built at. The drawn
    /// curve is the audio path, not a recomputed approximation.
    pub fn eq_response(&self, freqs: &[f32]) -> Vec<f32> {
        let rate = self.eq_rate.load(Ordering::Relaxed);
        let specs = self.eq_specs.lock().unwrap();
        freqs
            .iter()
            .map(|&f| {
                specs
                    .iter()
                    .flatten()
                    .map(|s| {
                        let b = if s.peaking {
                            Biquad::peaking_eq(rate, s.freq_hz, s.q, s.gain_db)
                        } else {
                            Biquad::low_shelf(rate, s.freq_hz, s.q, s.gain_db)
                        };
                        b.response_db(f, rate)
                    })
                    .sum()
            })
            .collect()
    }

    /// Normalized spectrum bands into a caller buffer — the hot-path FFI
    /// shape (raw f32 fill, no JSON at display rate).
    pub fn viz_bands(&self, out: &mut [f32]) -> usize {
        self.viz.lock().unwrap().spectrum.normalized_into(out)
    }
    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.clamp(0.0, 2.0), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        self.volume.load(Ordering::Relaxed)
    }

    /// Live per-band EQ specs — None means flat/unset for that slot.
    /// IPC + FFI read this; `set_band` is the write path.
    pub fn eq_specs(&self) -> Vec<Option<BandSpec>> {
        self.eq_specs.lock().unwrap().clone()
    }
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }
    /// A track is loaded and paused — resume continues, play would restart.
    pub fn can_resume(&self) -> bool {
        self.loaded.load(Ordering::Relaxed) && !self.playing.load(Ordering::Relaxed)
    }
    pub fn position_secs(&self) -> f32 {
        self.position_secs.load(Ordering::Relaxed)
    }

    /// Draw-ready viz snapshot — UI polls this at display rate.
    pub fn viz_snapshot(&self) -> (Vec<f32>, [f32; 2], bool) {
        let mut tap = self.viz.lock().unwrap();
        let (peak, _rms) = tap.levels.read();
        (
            SpectrumAnalyzer::normalized(&tap.spectrum.peek()),
            peak,
            tap.levels.take_clip(),
        )
    }

    /// Latest viz frame for the 60 Hz FFI path — short lock, memcpy out.
    /// Returns seq; the caller skips redraw when seq is unchanged.
    pub fn viz_frame(&self, out: &mut VizFrame) -> u64 {
        let tap = self.viz.lock().unwrap();
        *out = tap.frame;
        out.seq
    }

    pub fn shutdown(&self) {
        let _ = self.cmd.send(Command::Shutdown);
    }
}

/// Everything the decode/DSP worker thread owns — one struct instead of
/// a twelve-argument function, so the spawn site reads as configuration.
struct WorkerState {
    cmd: Receiver<Command>,
    prod: HeapProd<f32>,
    playing: Arc<AtomicBool>,
    loaded: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
    position: Arc<AtomicF32>,
    flush: Arc<AtomicBool>,
    viz: Arc<Mutex<VizTap>>,
    eq_specs: Arc<Mutex<Vec<Option<BandSpec>>>>,
    eq_rate: Arc<AtomicF32>,
    device_rate: f32,
    device_ch: usize,
    metrics: Arc<EngineMetrics>,
}

fn worker_loop(st: WorkerState) {
    let WorkerState {
        cmd,
        mut prod,
        playing,
        loaded,
        ended,
        position,
        flush,
        viz,
        eq_specs,
        eq_rate,
        device_rate,
        device_ch,
        metrics,
    } = st;
    let mut decoder: Option<TrackDecoder> = None;
    let mut eq = ParametricEq::default();
    let mut limiter = SafetyLimiter::new(44_100.0, 200.0);
    let mut src_rate = 44_100u32;
    let mut paused = false;
    let mut resampler: Option<rubato::Fft<f32>> = None;
    let mut pending_in: Vec<f32> = Vec::new(); // sub-chunk input for Fft
    let mut src_channels = 2usize;

    loop {
        // Drain pending commands (non-blocking while decoding).
        match if decoder.is_none() {
            cmd.recv().map(Some).unwrap_or(None)
        } else {
            cmd.try_recv().ok()
        } {
            Some(Command::Shutdown) => break,
            Some(Command::Play { source, extension }) => {
                let src = lyra_fs::SourceMediaSource::new(source);
                match TrackDecoder::open(src, extension.as_deref()) {
                    Ok(d) => {
                        src_rate = d.sample_rate;
                        src_channels = d.channels;
                        eq_rate.store(src_rate as f32, Ordering::Relaxed);
                        eq.bands.clear(); // flat until SetBand commands land
                        limiter = SafetyLimiter::new(src_rate as f32, 200.0);
                        // SRC when the track rate ≠ device rate (compat path).
                        resampler = if src_rate as f32 != device_rate {
                            rubato::Fft::<f32>::new(
                                src_rate as usize,
                                device_rate as usize,
                                1024,
                                2,
                                2,
                                rubato::FixedSync::Both,
                            )
                            .ok()
                        } else {
                            None
                        };
                        decoder = Some(d);
                        paused = false;
                        loaded.store(true, Ordering::Relaxed);
                        ended.store(false, Ordering::Relaxed);
                        flush.store(true, Ordering::Relaxed);
                        position.store(0.0, Ordering::Relaxed);
                        playing.store(true, Ordering::Relaxed);
                    }
                    Err(e) => warn!("decode open failed: {e}"),
                }
            }
            Some(Command::Pause) => {
                paused = true;
                playing.store(false, Ordering::Relaxed);
            }
            Some(Command::Resume) => {
                paused = false;
                // decoder may be None (whole track already in ring) — ended
                // means buffered audio remains; if the ring is also empty the
                // callback's drain-check turns playing back off.
                playing.store(
                    decoder.is_some() || ended.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
            }
            Some(Command::Stop) => {
                decoder = None;
                playing.store(false, Ordering::Relaxed);
                loaded.store(false, Ordering::Relaxed);
                ended.store(false, Ordering::Relaxed);
                flush.store(true, Ordering::Relaxed);
                position.store(0.0, Ordering::Relaxed);
            }
            Some(Command::Seek(secs)) => {
                if let Some(d) = decoder.as_mut() {
                    let _ = d.seek(secs);
                    flush.store(true, Ordering::Relaxed);
                    position.store(secs as f32, Ordering::Relaxed);
                }
            }
            Some(Command::SetBand(i, spec)) => {
                let flat = EqBand {
                    filter: Biquad::peaking_eq(src_rate as f32, 1000.0, 0.7, 0.0),
                    enabled: false,
                };
                if i >= eq.bands.len() && i < EQ_BANDS {
                    eq.bands.resize(i + 1, flat);
                }
                if let Some(b) = eq.bands.get_mut(i) {
                    let f = if spec.peaking {
                        Biquad::peaking_eq(src_rate as f32, spec.freq_hz, spec.q, spec.gain_db)
                    } else {
                        Biquad::low_shelf(src_rate as f32, spec.freq_hz, spec.q, spec.gain_db)
                    };
                    *b = EqBand {
                        filter: f,
                        enabled: spec.gain_db != 0.0,
                    };
                }
                if let Ok(mut specs) = eq_specs.try_lock() {
                    if i < specs.len() {
                        specs[i] = if spec.gain_db != 0.0 {
                            Some(spec)
                        } else {
                            None
                        };
                    }
                }
            }
            None => {}
        }

        if paused {
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        let Some(d) = decoder.as_mut() else {
            // idle — wait handled above via blocking recv
            thread::sleep(Duration::from_millis(5));
            continue;
        };

        match d.next_block() {
            Ok(Some(pcm_in)) => {
                metrics.inc_decode_blocks();
                // Channel adapt → stereo. Mono duplicates, >2ch keeps L/R.
                let mut pcm = adapt_channels(pcm_in, src_channels, device_ch);
                // SRC to device rate — always stereo after adapt.
                if let Some(rs) = resampler.as_mut() {
                    pcm = resample(rs, &pcm, &mut pending_in);
                }
                // DSP: EQ → safety limiter (catches EQ-induced clipping) → viz.
                eq.process(&mut pcm);
                limiter.process(&mut pcm);
                if let Ok(mut tap) = viz.try_lock() {
                    tap.push(&pcm);
                }
                // Push to ring; brief park when full (consumer drains at rate).
                let mut off = 0;
                while off < pcm.len() {
                    off += prod.push_slice(&pcm[off..]);
                    if off < pcm.len() {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
            Ok(None) => {
                // EOF — decoder done but the ring may still hold seconds of
                // audio. `ended` lets the callback drain it before idling.
                metrics.inc_eof();
                decoder = None;
                ended.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                metrics.inc_decode_errors();
                warn!("decode error: {e}");
                decoder = None;
                ended.store(true, Ordering::Relaxed); // drain what we have
            }
        }
    }
}

/// Adapt channel count: mono duplicates to every output channel, >2 keeps
/// the first `out_ch` channels. Ring always carries `device_ch` layout.
fn adapt_channels(pcm: Vec<f32>, src_ch: usize, out_ch: usize) -> Vec<f32> {
    if src_ch == out_ch {
        return pcm;
    }
    let frames = pcm.len() / src_ch;
    let mut out = Vec::with_capacity(frames * out_ch);
    for f in 0..frames {
        for c in 0..out_ch {
            out.push(pcm[f * src_ch + c.min(src_ch - 1)]);
        }
    }
    out
}

/// rubato Fft needs exactly `input_frames_next()` frames per call —
/// accumulate input across blocks in `pending`, process full chunks.
/// Output comes back interleaved via InterleavedOwned::take_data.
fn resample(rs: &mut rubato::Fft<f32>, interleaved: &[f32], pending: &mut Vec<f32>) -> Vec<f32> {
    use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
    use rubato::Resampler;
    pending.extend_from_slice(interleaved);
    let need_frames = rs.input_frames_next();
    let mut out = Vec::new();
    while pending.len() >= need_frames * 2 {
        let chunk: Vec<f32> = pending.drain(..need_frames * 2).collect();
        let l: Vec<f32> = chunk.iter().step_by(2).copied().collect();
        let r: Vec<f32> = chunk.iter().skip(1).step_by(2).copied().collect();
        match SequentialSliceOfVecs::new(&[l, r], 2, need_frames)
            .map_err(|e| e.to_string())
            .and_then(|a| rs.process(&a, 0, None).map_err(|e| e.to_string()))
        {
            Ok(res) => out.extend_from_slice(&res.take_data()),
            Err(e) => warn!("resample: {e}"),
        }
    }
    out
}
