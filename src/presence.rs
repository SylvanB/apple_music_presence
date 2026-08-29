//! Translates a `Track` into a Discord rich presence activity.

use std::time::{SystemTime, UNIX_EPOCH};

use discord_presence::models::Activity;

use crate::artwork::Artwork;
use crate::player::Track;

/// Discord rejects `state`/`details` outside 2..=128 characters.
const MIN_FIELD_CHARS: usize = 2;
const MAX_FIELD_CHARS: usize = 128;
const BUTTON_LABEL: &str = "Listen on Apple Music";

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Absolute start/end stamps for Discord's progress bar.
///
/// Anchoring `start` to the playhead rather than to "whenever we noticed" is
/// what makes the elapsed time survive seeking, pausing and app restarts.
pub fn timestamps(track: &Track, now: u64) -> Option<(u64, u64)> {
    if track.duration <= 0.0 {
        return None;
    }

    let position = track.position.clamp(0.0, track.duration).round() as u64;
    let start = now.saturating_sub(position);

    Some((start, start + track.duration.round() as u64))
}

/// Clamps a string into the range Discord accepts, or drops it entirely.
fn field(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut field: String = trimmed.chars().take(MAX_FIELD_CHARS).collect();
    while field.chars().count() < MIN_FIELD_CHARS {
        field.push(' ');
    }

    Some(field)
}

pub fn build_activity(
    activity: Activity,
    track: &Track,
    artwork: Option<&Artwork>,
    timestamps: Option<(u64, u64)>,
) -> Activity {
    let mut activity = activity;

    // Discord's convention: `details` is the headline, `state` the line under it.
    if let Some(details) = field(&track.name) {
        activity = activity.details(details);
    }
    if let Some(state) = field(&track.artist) {
        activity = activity.state(state);
    }

    let large_image = artwork.map(|art| art.image_url.clone());
    let large_text = field(&track.album);
    if large_image.is_some() || large_text.is_some() {
        activity = activity.assets(|mut assets| {
            if let Some(image) = large_image {
                assets = assets.large_image(image);
            }
            if let Some(text) = large_text {
                assets = assets.large_text(text);
            }
            assets
        });
    }

    if let Some((start, end)) = timestamps {
        activity = activity.timestamps(|stamps| stamps.start(start).end(end));
    }

    if let Some(url) = artwork.and_then(|art| art.track_url.clone()) {
        activity = activity.append_buttons(|button| button.label(BUTTON_LABEL).url(url));
    }

    activity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerState;

    fn boxer() -> Track {
        Track {
            name: "The Boxer".into(),
            artist: "Simon & Garfunkel".into(),
            album: "Bridge Over Troubled Water".into(),
            duration: 312.0,
            position: 41.0,
            player_state: PlayerState::Playing,
        }
    }

    fn artwork() -> Artwork {
        Artwork {
            image_url: "https://example.invalid/512x512bb.jpg".into(),
            track_url: Some("https://music.apple.com/us/album/the-boxer/1".into()),
        }
    }

    #[test]
    fn track_is_the_headline_and_artist_the_subtitle() {
        let activity = build_activity(Activity::new(), &boxer(), None, None);

        assert_eq!(activity.details.as_deref(), Some("The Boxer"));
        assert_eq!(activity.state.as_deref(), Some("Simon & Garfunkel"));
    }

    #[test]
    fn album_and_image_both_survive() {
        let art = artwork();
        let activity = build_activity(Activity::new(), &boxer(), Some(&art), None);
        let assets = activity.assets.expect("assets");

        assert_eq!(assets.large_image.as_deref(), Some(art.image_url.as_str()));
        assert_eq!(
            assets.large_text.as_deref(),
            Some("Bridge Over Troubled Water")
        );
    }

    #[test]
    fn adds_an_apple_music_button_when_a_link_is_known() {
        let art = artwork();
        let activity = build_activity(Activity::new(), &boxer(), Some(&art), None);

        assert_eq!(activity.buttons.len(), 1);
        assert_eq!(activity.buttons[0].label.as_deref(), Some(BUTTON_LABEL));
        assert_eq!(activity.buttons[0].url, art.track_url);
    }

    #[test]
    fn no_button_without_a_link() {
        let art = Artwork {
            track_url: None,
            ..artwork()
        };
        let activity = build_activity(Activity::new(), &boxer(), Some(&art), None);

        assert!(activity.buttons.is_empty());
    }

    #[test]
    fn timestamps_anchor_to_the_playhead() {
        let (start, end) = timestamps(&boxer(), 1_000).expect("timestamps");

        assert_eq!(start, 1_000 - 41);
        assert_eq!(end, start + 312);
    }

    #[test]
    fn timestamps_stay_put_as_the_track_plays() {
        let mut later = boxer();
        later.position = 141.0;

        // 100 seconds later, 100 seconds further in: the anchor must not move,
        // otherwise the progress bar restarts on every update.
        assert_eq!(timestamps(&boxer(), 1_000), timestamps(&later, 1_100));
    }

    #[test]
    fn a_seek_moves_the_anchor() {
        let mut sought = boxer();
        sought.position = 200.0;

        assert_ne!(timestamps(&boxer(), 1_000), timestamps(&sought, 1_000));
    }

    #[test]
    fn streams_without_a_duration_have_no_progress_bar() {
        let stream = Track {
            duration: 0.0,
            ..boxer()
        };

        assert!(timestamps(&stream, 1_000).is_none());
    }

    #[test]
    fn a_position_past_the_end_cannot_rewind_the_clock() {
        let overrun = Track {
            position: 9_999.0,
            ..boxer()
        };
        let (start, _) = timestamps(&overrun, 1_000).expect("timestamps");

        assert_eq!(start, 1_000 - 312);
    }

    #[test]
    fn short_fields_are_padded_and_long_ones_truncated() {
        assert_eq!(field("4").as_deref(), Some("4 "));
        assert_eq!(field("   ").as_deref(), None);
        assert_eq!(
            field("é".repeat(200).as_str()).unwrap().chars().count(),
            128
        );
    }

    #[test]
    fn empty_metadata_produces_an_empty_activity() {
        let activity = build_activity(Activity::new(), &Track::default(), None, None);

        assert!(activity.details.is_none());
        assert!(activity.state.is_none());
        assert!(activity.assets.is_none());
    }
}
