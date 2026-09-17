import AppKit
import Foundation
import MediaPlayer

/// Media keys + Control Center / lock-screen "Now Playing" integration.
///
/// The macOS trap (documented in research): the system cannot infer state
/// from an audio session — `playbackState` MUST be set explicitly, or keys
/// go to Music.app and Control Center shows nothing.
enum VolumeKey {
    case up, down, mute
}

final class MediaKeys {
    static let shared = MediaKeys()
    private var hooked = false

    /// Called once from the app on first track change; hooks the command
    /// center. Handlers call back into the VM/player.
    func hook(getState: @escaping () -> (playing: Bool, pos: Double),
              onToggle: @escaping () -> Void,
              onNext: @escaping () -> Void,
              onPrev: @escaping () -> Void,
              onSeek: @escaping (Double) -> Void,
              onVolume: @escaping (VolumeKey) -> Void) {
        guard !hooked else { return }
        hooked = true
        let cc = MPRemoteCommandCenter.shared()
        cc.togglePlayPauseCommand.addTarget { _ in onToggle(); return .success }
        cc.playCommand.addTarget { _ in onToggle(); return .success }
        cc.pauseCommand.addTarget { _ in onToggle(); return .success }
        cc.nextTrackCommand.addTarget { _ in onNext(); return .success }
        cc.previousTrackCommand.addTarget { _ in onPrev(); return .success }
        cc.changePlaybackPositionCommand.addTarget { e in
            guard let e = e as? MPChangePlaybackPositionCommandEvent else {
                return .commandFailed
            }
            onSeek(e.positionTime)
            return .success
        }
        self.getState = getState

        // Hardware volume keys (F11/F12/mute) arrive as .systemDefined
        // events with the aux-buttons subtype. Under exclusive HAL output
        // the system mixer is bypassed, so they must steer our engine
        // volume — a local monitor covers the focused window case; the
        // system bezel still updates cosmetically either way.
        NSEvent.addLocalMonitorForEvents(matching: .systemDefined) { e in
            // subtype 8 = aux control buttons; data1 packs
            // (keyCode << 16) | (state << 8) | repeat — 0xA = key-down.
            guard e.subtype.rawValue == 8 else { return e }
            let keyCode = (e.data1 & 0xFFFF_0000) >> 16
            let keyDown = ((e.data1 & 0xFF00) >> 8) == 0xA
            let repeat_ = (e.data1 & 0x1) != 0
            guard keyDown || repeat_ else { return e }
            switch keyCode {
            case 0: onVolume(.up)      // NX_KEYTYPE_SOUND_UP
            case 1: onVolume(.down)    // NX_KEYTYPE_SOUND_DOWN
            case 7: onVolume(.mute)    // NX_KEYTYPE_MUTE
            default: return e
            }
            return e // let the system bezel update too
        }
    }

    private var getState: (() -> (playing: Bool, pos: Double))?

    /// Publish track metadata + live state — call on track change, seek,
    /// and play/pause transitions.
    func publish(title: String, artist: String, album: String, duration: Double,
                 artworkHash: String? = nil) {
        let (playing, pos) = getState?() ?? (false, 0)
        var info: [String: Any] = [
            MPMediaItemPropertyTitle: title,
            MPMediaItemPropertyArtist: artist,
            MPMediaItemPropertyAlbumTitle: album,
            MPMediaItemPropertyPlaybackDuration: duration,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: pos,
            MPNowPlayingInfoPropertyPlaybackRate: playing ? 1.0 : 0.0,
        ]
        info[MPMediaItemPropertyMediaType] = MPNowPlayingInfoMediaType.audio.rawValue
        if let img = Artwork.image(artworkHash, size: 256) {
            info[MPMediaItemPropertyArtwork] = MPMediaItemArtwork(
                boundsSize: img.size) { _ in img }
        }
        MPNowPlayingInfoCenter.default().nowPlayingInfo = info
        // THE line that makes macOS hand us the media keys:
        MPNowPlayingInfoCenter.default().playbackState = playing ? .playing : .paused
    }

    /// Lightweight state-only update (position tick / play-pause toggle).
    func refreshState() {
        let (playing, pos) = getState?() ?? (false, 0)
        MPNowPlayingInfoCenter.default().playbackState = playing ? .playing : .paused
        MPNowPlayingInfoCenter.default().nowPlayingInfo?[MPNowPlayingInfoPropertyElapsedPlaybackTime] = pos
    }
}
