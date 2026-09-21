//! lyra-hal: the audiophile output path BitMuse doesn't have.
//!
//! Thin safe layer over the CoreAudio HAL device API:
//!  - default output device, name, nominal sample rate get/set
//!  - hog mode (exclusive access) as an RAII guard
//!  - available nominal rates, physical/virtual stream formats
//!  - IOProc output: caller supplies `FnMut(&mut [f32]) -> usize` pulling
//!    interleaved stereo f32 — the same contract the engine's ring buffer
//!    already satisfies. Deinterleaves into device buffers per frame.
//!
//! Rate-switch sequencing per BLUEPRINT.md: set nominal rate → wait for it
//! to apply → start IOProc. Hog first so nothing else reconfigures the
//! device mid-switch.
//!
//! macOS-only: CoreAudio has no Linux counterpart. The crate compiles to an
//! empty stub elsewhere; callers gate on `cfg(target_os = "macos")` and the
//! engine's cpal compat path is the portable driver.

#![cfg(target_os = "macos")]

use coreaudio_sys::*;
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {}

use lyra_core::LyraError;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Pull callback for IOProc output: fills `dst` with interleaved stereo
/// f32, returns frames written (0 = silence/underrun).
type PullCallback = Box<dyn FnMut(&mut [f32]) -> usize + Send>;

fn os_err(op: &str, status: OSStatus) -> LyraError {
    LyraError::Audio(format!(
        "{op}: OSStatus {status} ({:p})",
        status as *const c_void
    ))
}

fn addr(
    selector: AudioObjectPropertySelector,
    scope: u32,
    element: u32,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: element,
    }
}

/// Get a property whose value fits in one POD.
fn get_prop<T>(obj: AudioObjectID, a: &AudioObjectPropertyAddress) -> Result<T, LyraError> {
    let mut v: T = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<T>() as u32;
    let st = unsafe {
        AudioObjectGetPropertyData(
            obj,
            a,
            0,
            std::ptr::null(),
            &mut size,
            &mut v as *mut T as *mut c_void,
        )
    };
    if st != 0 {
        return Err(os_err("get prop", st));
    }
    Ok(v)
}

fn set_prop<T>(obj: AudioObjectID, a: &AudioObjectPropertyAddress, v: &T) -> Result<(), LyraError> {
    let st = unsafe {
        AudioObjectSetPropertyData(
            obj,
            a,
            0,
            std::ptr::null(),
            std::mem::size_of::<T>() as u32,
            v as *const T as *const c_void,
        )
    };
    if st != 0 {
        return Err(os_err("set prop", st));
    }
    Ok(())
}

/// A CoreAudio output device.
pub struct HalDevice {
    pub id: AudioDeviceID,
}

impl HalDevice {
    pub fn default_output() -> Result<Self, LyraError> {
        let id: AudioDeviceID = get_prop(
            kAudioObjectSystemObject as AudioObjectID,
            &addr(
                kAudioHardwarePropertyDefaultOutputDevice,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            ),
        )?;
        if id == kAudioObjectUnknown as AudioDeviceID {
            return Err(LyraError::Audio("no default output device".into()));
        }
        Ok(Self { id })
    }

    /// Device UID (stable across reconnects — for "prefer this DAC" prefs).
    pub fn uid(&self) -> Result<String, LyraError> {
        self.cfstring_prop(kAudioDevicePropertyDeviceUID)
    }

    pub fn name(&self) -> Result<String, LyraError> {
        self.cfstring_prop(kAudioObjectPropertyName)
    }

    fn cfstring_prop(&self, sel: AudioObjectPropertySelector) -> Result<String, LyraError> {
        let cf: CFStringRef = get_prop(
            self.id,
            &addr(
                sel,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            ),
        )?;
        if cf.is_null() {
            return Err(LyraError::Audio("null CFString".into()));
        }
        let len = unsafe { CFStringGetLength(cf) };
        let mut buf = vec![0u16; len as usize + 1];
        unsafe {
            CFStringGetCharacters(
                cf,
                CFRange {
                    location: 0,
                    length: len,
                },
                buf.as_mut_ptr(),
            );
            CFRelease(cf as *const c_void);
        }
        Ok(String::from_utf16_lossy(&buf[..len as usize]))
    }

    pub fn nominal_rate(&self) -> Result<f64, LyraError> {
        get_prop(
            self.id,
            &addr(
                kAudioDevicePropertyNominalSampleRate,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            ),
        )
    }

    /// Supported nominal-rate ranges [(min,max)] — 44100/48000/88200/…
    pub fn available_rates(&self) -> Result<Vec<(f64, f64)>, LyraError> {
        let a = addr(
            kAudioDevicePropertyAvailableNominalSampleRates,
            kAudioObjectPropertyScopeGlobal,
            kAudioObjectPropertyElementMain,
        );
        let mut size = 0u32;
        let st =
            unsafe { AudioObjectGetPropertyDataSize(self.id, &a, 0, std::ptr::null(), &mut size) };
        if st != 0 {
            return Err(os_err("rate list size", st));
        }
        let n = size as usize / std::mem::size_of::<AudioValueRange>();
        let mut ranges = vec![
            AudioValueRange {
                mMinimum: 0.0,
                mMaximum: 0.0
            };
            n
        ];
        let st = unsafe {
            AudioObjectGetPropertyData(
                self.id,
                &a,
                0,
                std::ptr::null(),
                &mut size,
                ranges.as_mut_ptr() as *mut c_void,
            )
        };
        if st != 0 {
            return Err(os_err("rate list", st));
        }
        Ok(ranges.iter().map(|r| (r.mMinimum, r.mMaximum)).collect())
    }

    /// Switch the device's nominal rate and block until it applies
    /// (hardware needs tens of ms — callers must not start IOProc before
    /// this returns or the first buffers play at the old rate).
    pub fn set_nominal_rate(&self, rate: f64) -> Result<(), LyraError> {
        if (self.nominal_rate()? - rate).abs() < 0.5 {
            return Ok(());
        }
        set_prop(
            self.id,
            &addr(
                kAudioDevicePropertyNominalSampleRate,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            ),
            &rate,
        )?;
        for _ in 0..100 {
            if (self.nominal_rate()? - rate).abs() < 0.5 {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Err(LyraError::Audio(format!("rate never settled to {rate}")))
    }

    /// Exclusive access — device won't accept other clients' streams while
    /// held. Drop releases (and on process exit the HAL reclaims anyway).
    /// Snapshots the system mixer first: while we own the device, volume
    /// keys still write the (bypassed) master volume/mute — invisible drift
    /// the user would discover when the hog drops. Restored on release.
    pub fn hog(&self) -> Result<Hog, LyraError> {
        let a = addr(
            kAudioDevicePropertyHogMode,
            kAudioObjectPropertyScopeGlobal,
            kAudioObjectPropertyElementMain,
        );
        let holder: i32 = get_prop(self.id, &a)?;
        let me = std::process::id() as i32;
        if holder != me && holder != -1 {
            return Err(LyraError::Audio(format!("device hogged by pid {holder}")));
        }
        let saved_vol: Option<f32> = get_prop(
            self.id,
            &addr(
                kAudioDevicePropertyVolumeScalar,
                kAudioObjectPropertyScopeOutput,
                kAudioObjectPropertyElementMain,
            ),
        )
        .ok();
        let saved_mute: Option<u32> = get_prop(
            self.id,
            &addr(
                kAudioDevicePropertyMute,
                kAudioObjectPropertyScopeOutput,
                kAudioObjectPropertyElementMain,
            ),
        )
        .ok();
        if holder != me {
            set_prop(self.id, &a, &me)?;
        }
        Ok(Hog {
            dev: self.id,
            released: AtomicBool::new(false),
            saved_vol,
            saved_mute,
        })
    }

    /// Virtual output format: (channels, sample_rate, interleaved).
    pub fn virtual_format(&self) -> Result<(usize, f64, bool), LyraError> {
        let f: AudioStreamBasicDescription = get_prop(
            self.id,
            &addr(
                kAudioDevicePropertyStreamFormat,
                kAudioDevicePropertyScopeOutput,
                kAudioObjectPropertyElementMain,
            ),
        )?;
        Ok((
            f.mChannelsPerFrame as usize,
            f.mSampleRate,
            f.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0,
        ))
    }

    /// IOProc output. `pull` fills `dst` with interleaved stereo f32 and
    /// returns frames written (0 = silence/underrun). Runs on the HAL's
    /// real-time thread — `pull` must be lock-free (ring buffer).
    pub fn start_ioproc(&self, pull: PullCallback) -> Result<IoProc, LyraError> {
        // Query the virtual format once — channel count and interleaved
        // layout drive the copy path (non-interleaved is the common macOS
        // default: one buffer per channel).
        let fmt: AudioStreamBasicDescription = get_prop(
            self.id,
            &addr(
                kAudioDevicePropertyStreamFormat,
                kAudioDevicePropertyScopeOutput,
                kAudioObjectPropertyElementMain,
            ),
        )?;
        let interleaved = fmt.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0;
        let channels = fmt.mChannelsPerFrame as usize;
        // Scratch sized for the device's max callback — no alloc on the
        // RT thread. If a callback ever arrives bigger we clip (silence
        // tails the excess) rather than allocate.
        let max_frames: u32 = get_prop(
            self.id,
            &addr(
                kAudioDevicePropertyBufferFrameSize,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            ),
        )
        .unwrap_or(4096)
        .max(4096);
        let ctx = Box::new(IoProcCtx {
            pull: Mutex::new((pull, vec![0f32; max_frames as usize * channels.max(1)])),
            underruns: std::sync::atomic::AtomicU64::new(0),
            channels: channels.max(1),
            interleaved,
        });
        let ctx_ptr = Box::into_raw(ctx);
        extern "C" fn trampoline(
            _dev: AudioDeviceID,
            _now: *const AudioTimeStamp,
            in_data: *const AudioBufferList,
            _in_time: *const AudioTimeStamp,
            out_data: *mut AudioBufferList,
            _out_time: *const AudioTimeStamp,
            client: *mut c_void,
        ) -> OSStatus {
            let ctx = unsafe { &*(client as *const IoProcCtx) };
            let list = unsafe { &mut *out_data };
            let n_bufs = list.mNumberBuffers as usize;
            let bufs =
                unsafe { std::slice::from_raw_parts_mut(list.mBuffers.as_mut_ptr(), n_bufs) };
            if bufs.is_empty() {
                return 0;
            }
            let channels = ctx.channels;
            let bytes_per_frame = if ctx.interleaved { 4 * channels } else { 4 };
            let frames = bufs[0].mDataByteSize as usize / bytes_per_frame;
            let mut guard = match ctx.pull.try_lock() {
                Ok(g) => g,
                Err(_) => return 0,
            };
            let (pull, tmp) = &mut *guard;
            let cap = tmp.len() / channels;
            let frames = frames.min(cap); // clip over-size callbacks
            let written = pull(&mut tmp[..frames * channels]);
            // Idle passthrough: while the engine has no audio, forward
            // inInputData (every other client's mixed output) verbatim.
            // An output IOProc owns the hardware path — writing silence
            // here would mute the whole system even with hog released.
            if written == usize::MAX {
                if in_data.is_null() {
                    for b in bufs.iter_mut() {
                        unsafe { std::ptr::write_bytes(b.mData, 0, b.mDataByteSize as usize) };
                    }
                    return 0;
                }
                let in_list = unsafe { &*in_data };
                let in_bufs = unsafe {
                    std::slice::from_raw_parts(
                        in_list.mBuffers.as_ptr(),
                        in_list.mNumberBuffers as usize,
                    )
                };
                for (i, b) in bufs.iter_mut().enumerate() {
                    let (dst, dst_len) = (b.mData as *mut u8, b.mDataByteSize as usize);
                    match in_bufs.get(i) {
                        Some(src) if !src.mData.is_null() => {
                            let n = dst_len.min(src.mDataByteSize as usize);
                            unsafe {
                                std::ptr::copy_nonoverlapping(src.mData as *const u8, dst, n);
                                if dst_len > n {
                                    std::ptr::write_bytes(dst.add(n), 0, dst_len - n);
                                }
                            }
                        }
                        _ => unsafe { std::ptr::write_bytes(dst, 0, dst_len) },
                    }
                }
                return 0;
            }
            // Shortfall = underrun (idle already returned via the
            // passthrough branch above).
            if written < frames {
                ctx.underruns.fetch_add(1, Ordering::Relaxed);
            }
            let filled = written.min(frames);
            if ctx.interleaved {
                let dst = unsafe {
                    std::slice::from_raw_parts_mut(bufs[0].mData as *mut f32, frames * channels)
                };
                dst.copy_from_slice(&tmp[..dst.len()]);
            } else {
                for (ch, b) in bufs.iter_mut().enumerate().take(channels) {
                    let dst =
                        unsafe { std::slice::from_raw_parts_mut(b.mData as *mut f32, frames) };
                    for (f, d) in dst.iter_mut().enumerate() {
                        *d = if f < filled {
                            tmp[f * channels + ch]
                        } else {
                            0.0
                        };
                    }
                }
            }
            0
        }
        let mut proc_id: AudioDeviceIOProcID = None;
        let st = unsafe {
            AudioDeviceCreateIOProcID(
                self.id,
                Some(trampoline),
                ctx_ptr as *mut c_void,
                &mut proc_id,
            )
        };
        if st != 0 {
            unsafe { drop(Box::from_raw(ctx_ptr)) };
            return Err(os_err("create IOProc", st));
        }
        // Start can return EAGAIN while the device is still settling from
        // a rate/format transition — retry briefly before giving up.
        for attempt in 0..50 {
            let st = unsafe { AudioDeviceStart(self.id, proc_id) };
            if st == 0 {
                break;
            }
            if st != 35 || attempt == 49 {
                unsafe {
                    AudioDeviceDestroyIOProcID(self.id, proc_id);
                    drop(Box::from_raw(ctx_ptr));
                }
                return Err(os_err("start IOProc", st));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(IoProc {
            dev: self.id,
            proc_id,
            ctx: ctx_ptr,
        })
    }
}

/// RAII hog-mode guard — releases on drop, restoring the system mixer
/// snapshot taken at acquisition.
pub struct Hog {
    dev: AudioDeviceID,
    released: AtomicBool,
    saved_vol: Option<f32>,
    saved_mute: Option<u32>,
}

impl Hog {
    pub fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            let free: i32 = -1;
            let _ = set_prop(
                self.dev,
                &addr(
                    kAudioDevicePropertyHogMode,
                    kAudioObjectPropertyScopeGlobal,
                    kAudioObjectPropertyElementMain,
                ),
                &free,
            );
            if let Some(v) = self.saved_vol {
                let _ = set_prop(
                    self.dev,
                    &addr(
                        kAudioDevicePropertyVolumeScalar,
                        kAudioObjectPropertyScopeOutput,
                        kAudioObjectPropertyElementMain,
                    ),
                    &v,
                );
            }
            if let Some(m) = self.saved_mute {
                let _ = set_prop(
                    self.dev,
                    &addr(
                        kAudioDevicePropertyMute,
                        kAudioObjectPropertyScopeOutput,
                        kAudioObjectPropertyElementMain,
                    ),
                    &m,
                );
            }
        }
    }
}

impl Drop for Hog {
    fn drop(&mut self) {
        self.release();
    }
}

struct IoProcCtx {
    pull: Mutex<(PullCallback, Vec<f32>)>,
    underruns: std::sync::atomic::AtomicU64,
    channels: usize,
    interleaved: bool,
}

/// A running IOProc — stop + destroy on drop.
pub struct IoProc {
    dev: AudioDeviceID,
    proc_id: AudioDeviceIOProcID,
    ctx: *mut IoProcCtx,
}

impl IoProc {
    /// Callbacks that hit an empty ring (starvation counter).
    pub fn underruns(&self) -> u64 {
        unsafe { &*self.ctx }.underruns.load(Ordering::Relaxed)
    }
}

impl Drop for IoProc {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.dev, self.proc_id);
            AudioDeviceDestroyIOProcID(self.dev, self.proc_id);
            drop(Box::from_raw(self.ctx));
        }
    }
}
