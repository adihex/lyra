import Foundation
import MediaPlayer

/// Media keys + Control Center / lock-screen "Now Playing" integration.
///
/// The macOS trap (documented in research): the system cannot infer state
/// from an audio session — `playbackState` MUST be set explicitly, or keys
/// go to Music.app and Control Center shows nothing.
final class MediaKeys {
    static let shared = MediaKeys()
    private var hooked = false

    /// Called once from the app on first track change; hooks the command
    /// center. Handlers call back into the VM/player.
    func hook(getState: @escaping () -> (playing: Bool, pos: Double),
              onToggle: @escaping () -> Void,
              onNext: @escaping () -> Void,
              onPrev: @escaping () -> Void,
              onSeek: @escaping (Double) -> Void) {
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
    }

    private var getState: (() -> (playing: Bool, pos: Double))?

    /// Publish track metadata + live state — call on track change, seek,
    /// and play/pause transitions.
    func publish(title: String, artist: String, album: String, duration: Double) {
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
