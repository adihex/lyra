//! Hex-pickup / MIDI-guitar input — the trivially-accurate judging path.
//!
//! Behind the off-by-default `midi` feature (MIT-licensed `midir`,
//! CoreMIDI on macOS / ALSA on Linux). Owners of MIDI guitars (Jamstik,
//! Fishman TriplePlay, Sonuus) get exact pitch + velocity for free:
//! note-ons feed [`Session::midi_note_on`](crate::Session::midi_note_on)
//! with clarity 1.0, and timing is the only thing left to judge.
//!
//! This module never touches the audio thread: the OS delivers MIDI on
//! its own callback, which appends to a lock-guarded queue drained from
//! the game/UI thread. Timestamps are host seconds, not stream-clock —
//! reconcile against the audio clock through the normal calibration
//! offset before judging.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// One parsed MIDI note event. `channel` is 0–15 (hex pickups typically
/// send one string per channel — the future string-disambiguation hook).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiNote {
    pub note: u8,
    pub velocity: u8,
    pub on: bool,
    pub channel: u8,
}

/// Open MIDI input, queueing note events for polling.
pub struct MidiInput {
    // Held so the OS callback stays alive; never read after connect.
    _conn: midir::MidiInputConnection<Arc<Mutex<VecDeque<MidiNote>>>>,
    queue: Arc<Mutex<VecDeque<MidiNote>>>,
    port_name: String,
}

impl MidiInput {
    /// Open the first port whose name contains `name_match` (or the first
    /// port at all when `None`). Returns an error listing ports when
    /// nothing matches.
    pub fn open(name_match: Option<&str>) -> Result<Self, String> {
        let input = midir::MidiInput::new("lyra-coach").map_err(|e| e.to_string())?;
        let ports = input.ports();
        if ports.is_empty() {
            return Err("no MIDI input ports found".to_string());
        }
        let port = ports
            .iter()
            .find(|p| {
                name_match.map_or(true, |want| {
                    input
                        .port_name(p)
                        .map(|n| n.contains(want))
                        .unwrap_or(false)
                })
            })
            .ok_or_else(|| {
                let names: Vec<String> = ports
                    .iter()
                    .map(|p| input.port_name(p).unwrap_or_default())
                    .collect();
                format!("no MIDI port matching {name_match:?}; have {names:?}")
            })?;
        let port_name = input.port_name(port).unwrap_or_default();
        let queue: Arc<Mutex<VecDeque<MidiNote>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(64)));
        let sink = Arc::clone(&queue);
        let conn = input
            .connect(
                port,
                "lyra-coach-in",
                move |_stamp, msg, sink: &mut Arc<Mutex<VecDeque<MidiNote>>>| {
                    if let Some(ev) = parse(msg) {
                        if let Ok(mut q) = sink.lock() {
                            // Bounded: drop oldest when the UI thread
                            // hasn't drained (prevents unbounded growth).
                            if q.len() >= 256 {
                                q.pop_front();
                            }
                            q.push_back(ev);
                        }
                    }
                },
                sink,
            )
            .map_err(|e| e.to_string())?;
        Ok(MidiInput {
            _conn: conn,
            queue,
            port_name,
        })
    }

    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// Drain queued note events (call from the game/UI thread, then feed
    /// note-ons to `Session::midi_note_on`).
    pub fn drain(&self) -> Vec<MidiNote> {
        self.queue
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }
}

/// Parse note-on/off (all 16 channels). Velocity-0 note-on = note-off.
fn parse(msg: &[u8]) -> Option<MidiNote> {
    if msg.len() < 3 {
        return None;
    }
    let (status, note, vel) = (msg[0], msg[1], msg[2]);
    match status & 0xF0 {
        0x90 => Some(MidiNote {
            note,
            velocity: vel,
            on: vel != 0,
            channel: status & 0x0F,
        }),
        0x80 => Some(MidiNote {
            note,
            velocity: vel,
            on: false,
            channel: status & 0x0F,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_note_on_off_across_channels() {
        let on = parse(&[0x95, 64, 100]).unwrap();
        assert!(on.on && on.note == 64 && on.velocity == 100 && on.channel == 5);
        assert!(!parse(&[0x95, 64, 0]).unwrap().on); // vel-0 on = off
        assert!(!parse(&[0x83, 64, 64]).unwrap().on);
        assert!(parse(&[0xB0, 7, 100]).is_none()); // CC ignored
        assert!(parse(&[0x90, 64]).is_none()); // short msg ignored
    }
}
