//! Reads the current Apple Music state via JavaScript for Automation (JXA).

use std::process::Command;

use serde::{Deserialize, Deserializer};
use tracing::debug;

/// The JXA snippet handed to `osascript`.
///
/// `running()` is deliberately checked first: touching any other property of
/// the Music application (`playerState`, `currentTrack`, ...) *launches*
/// Music.app, which a background presence daemon should never do.
const PLAYER_STATE_SCRIPT: &str = r#"
    const Music = Application("Music");
    const output = {};
    if (!Music.running()) {
      output.playerState = "not running";
    } else {
      output.playerState = Music.playerState();
      if (output.playerState === "playing" || output.playerState === "paused") {
        const track = Music.currentTrack;
        output.trackName = track.name();
        output.artistName = track.artist();
        output.albumName = track.album();
        output.duration = track.duration() || 0;
        output.position = Music.playerPosition() || 0;
        // "loved" was renamed "favorited"; older systems only have the former.
        try {
          output.favorited = track.favorited();
        } catch (e) {
          try {
            output.favorited = track.loved();
          } catch (e2) {
            output.favorited = false;
          }
        }
      }
    }
    JSON.stringify(output)
"#;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlayerState {
    Playing,
    Paused,
    #[default]
    Stopped,
    NotRunning,
}

impl PlayerState {
    /// Maps the strings Music.app reports onto our four states. Scrubbing
    /// counts as playing; anything unrecognised is treated as stopped, which
    /// is the state that simply clears the presence.
    fn from_music_app(raw: &str) -> Self {
        match raw {
            "playing" | "fast forwarding" | "rewinding" => Self::Playing,
            "paused" => Self::Paused,
            "not running" => Self::NotRunning,
            "stopped" => Self::Stopped,
            other => {
                debug!("unrecognised player state {other:?}, treating as stopped");
                Self::Stopped
            }
        }
    }

    pub fn has_track(self) -> bool {
        matches!(self, Self::Playing | Self::Paused)
    }
}

impl<'de> Deserialize<'de> for PlayerState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_music_app(&raw))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Track {
    #[serde(default, rename = "trackName")]
    pub name: String,
    #[serde(default, rename = "artistName")]
    pub artist: String,
    #[serde(default, rename = "albumName")]
    pub album: String,
    /// Track length in seconds.
    #[serde(default)]
    pub duration: f64,
    /// Playhead offset in seconds.
    #[serde(default)]
    pub position: f64,
    #[serde(default, rename = "playerState")]
    pub player_state: PlayerState,
    #[serde(default)]
    pub favorited: bool,
}

impl Track {
    /// Whether two polls refer to the same recording. Deliberately ignores
    /// `position`, which changes on every poll, and `player_state`, which the
    /// main loop reacts to separately.
    pub fn is_same_recording(&self, other: &Self) -> bool {
        self.name == other.name && self.artist == other.artist && self.album == other.album
    }

    /// Polls Music.app.
    ///
    /// The error is returned rather than logged: a revoked Automation
    /// permission fails on every single poll, and only the caller knows how
    /// often it has already complained.
    pub fn from_apple_music() -> Result<Self, String> {
        let output = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", PLAYER_STATE_SCRIPT])
            .output()
            .map_err(|err| format!("could not run osascript: {err}"))?;

        if !output.status.success() {
            return Err(format!(
                "osascript exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(stdout.trim())
            .map_err(|err| format!("could not parse player state {:?}: {err}", stdout.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_playing_track() {
        let track: Track = serde_json::from_str(
            r#"{"trackName":"The Boxer","artistName":"Simon & Garfunkel",
                "albumName":"Bridge Over Troubled Water","duration":312.5,
                "position":41.25,"playerState":"playing"}"#,
        )
        .unwrap();

        assert_eq!(track.name, "The Boxer");
        assert_eq!(track.artist, "Simon & Garfunkel");
        assert_eq!(track.player_state, PlayerState::Playing);
        assert_eq!(track.position, 41.25);
    }

    #[test]
    fn parses_the_favourite_flag() {
        let loved: Track = serde_json::from_str(
            r#"{"trackName":"The Boxer","playerState":"playing","favorited":true}"#,
        )
        .unwrap();
        assert!(loved.favorited);

        // Systems too old for either property simply omit it.
        let unknown: Track =
            serde_json::from_str(r#"{"trackName":"The Boxer","playerState":"playing"}"#).unwrap();
        assert!(!unknown.favorited);
    }

    #[test]
    fn a_failed_poll_reports_why() {
        // The caller rate-limits the logging, so the message must come back out
        // rather than being swallowed here.
        let err = serde_json::from_str::<Track>("not json").unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn parses_the_not_running_payload() {
        let track: Track = serde_json::from_str(r#"{"playerState":"not running"}"#).unwrap();

        assert_eq!(track.player_state, PlayerState::NotRunning);
        assert!(!track.player_state.has_track());
        assert!(track.name.is_empty());
    }

    #[test]
    fn scrubbing_counts_as_playing() {
        assert_eq!(
            PlayerState::from_music_app("fast forwarding"),
            PlayerState::Playing
        );
        assert_eq!(PlayerState::from_music_app("paused"), PlayerState::Paused);
        assert_eq!(PlayerState::from_music_app("wat"), PlayerState::Stopped);
    }

    #[test]
    fn same_recording_ignores_position() {
        let a = Track {
            name: "The Boxer".into(),
            artist: "Simon & Garfunkel".into(),
            album: "Bridge Over Troubled Water".into(),
            position: 10.0,
            ..Track::default()
        };
        let b = Track {
            position: 90.0,
            ..a.clone()
        };
        let c = Track {
            album: "Greatest Hits".into(),
            ..a.clone()
        };

        assert!(a.is_same_recording(&b));
        assert!(!a.is_same_recording(&c));
    }
}
