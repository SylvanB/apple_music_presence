mod artwork;
mod player;
mod presence;

use std::thread::sleep;
use std::time::Duration;

use discord_presence::Client;
use tracing::{error, info, warn};

use artwork::ArtworkClient;
use player::{PlayerState, Track};

const APPLE_MUSIC_CLIENT_ID: u64 = 773825528921849856;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How far the computed start anchor may drift before we call it a seek rather
/// than ordinary rounding between two polls.
const SEEK_TOLERANCE_SECS: u64 = 2;

/// What we last pushed to Discord, so we only speak to it when something
/// actually changed. Discord rate-limits presence updates, and steady playback
/// should produce no traffic at all.
struct Shown {
    track: Track,
    state: PlayerState,
    start: Option<u64>,
}

fn needs_update(shown: Option<&Shown>, track: &Track, start: Option<u64>) -> bool {
    let Some(shown) = shown else {
        return true;
    };

    if shown.state != track.player_state || !shown.track.is_same_recording(track) {
        return true;
    }

    match (shown.start, start) {
        // Ordinary playback advances the playhead exactly as fast as the clock,
        // so the anchor holds steady; a seek is what shifts it.
        (Some(previous), Some(current)) => previous.abs_diff(current) > SEEK_TOLERANCE_SECS,
        (None, None) => false,
        _ => true,
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let mut drpc = Client::new(APPLE_MUSIC_CLIENT_ID);

    drpc.on_ready(|_ctx| info!("connected to Discord"))
        .persist();
    drpc.on_error(|ctx| error!("discord error: {:?}", ctx.event))
        .persist();

    drpc.start();

    if let Err(err) = drpc.block_until_event(discord_presence::Event::Ready) {
        error!("never became ready: {err}");
        return;
    }

    let mut artwork = ArtworkClient::new();
    let mut shown: Option<Shown> = None;

    loop {
        sleep(POLL_INTERVAL);

        // A failed poll is transient: keep whatever is on screen.
        let Some(track) = Track::from_apple_music() else {
            continue;
        };

        if !track.player_state.has_track() {
            if shown.is_some() {
                match drpc.clear_activity() {
                    Ok(_) => {
                        info!("nothing playing, presence cleared");
                        shown = None;
                    }
                    Err(err) => warn!("failed to clear presence: {err}"),
                }
            }
            continue;
        }

        // Only a playing track gets a progress bar; a paused one sits still.
        let stamps = match track.player_state {
            PlayerState::Playing => presence::timestamps(&track, presence::unix_now()),
            _ => None,
        };
        let start = stamps.map(|(start, _)| start);

        if !needs_update(shown.as_ref(), &track, start) {
            continue;
        }

        let art = artwork.lookup(&track);
        info!(
            "{:?}: {} - {}",
            track.player_state, track.artist, track.name
        );

        if let Err(err) = drpc.set_activity(|activity| {
            presence::build_activity(activity, &track, art.as_ref(), stamps)
        }) {
            // Leave `shown` untouched so the next poll retries this update.
            warn!("failed to set presence: {err}");
            continue;
        }

        shown = Some(Shown {
            state: track.player_state,
            start,
            track,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> Track {
        Track {
            name: "The Boxer".into(),
            artist: "Simon & Garfunkel".into(),
            album: "Bridge Over Troubled Water".into(),
            duration: 312.0,
            position: 41.0,
            player_state: PlayerState::Playing,
        }
    }

    fn shown_from(track: &Track, start: Option<u64>) -> Shown {
        Shown {
            track: track.clone(),
            state: track.player_state,
            start,
        }
    }

    #[test]
    fn first_track_always_updates() {
        assert!(needs_update(None, &playing(), Some(959)));
    }

    #[test]
    fn steady_playback_is_silent() {
        let track = playing();
        let shown = shown_from(&track, Some(959));

        assert!(!needs_update(Some(&shown), &track, Some(959)));
        // One second of rounding slop must not trigger a new update.
        assert!(!needs_update(Some(&shown), &track, Some(960)));
    }

    #[test]
    fn seeking_updates() {
        let track = playing();
        let shown = shown_from(&track, Some(959));

        assert!(needs_update(Some(&shown), &track, Some(800)));
    }

    #[test]
    fn pausing_updates() {
        let track = playing();
        let shown = shown_from(&track, Some(959));

        let paused = Track {
            player_state: PlayerState::Paused,
            ..track
        };
        assert!(needs_update(Some(&shown), &paused, None));
    }

    #[test]
    fn resuming_from_pause_updates() {
        let paused = Track {
            player_state: PlayerState::Paused,
            ..playing()
        };
        let shown = shown_from(&paused, None);

        assert!(needs_update(Some(&shown), &playing(), Some(959)));
    }

    #[test]
    fn changing_track_updates() {
        let track = playing();
        let shown = shown_from(&track, Some(959));

        let next = Track {
            name: "Cecilia".into(),
            ..track
        };
        assert!(needs_update(Some(&shown), &next, Some(959)));
    }

    #[test]
    fn repeating_the_same_track_updates() {
        let track = playing();
        let shown = shown_from(&track, Some(959));

        // Same recording from the top: the anchor jumps forward by the elapsed
        // playtime, which is exactly what a restart looks like.
        let restarted = Track {
            position: 0.0,
            ..track
        };
        assert!(needs_update(Some(&shown), &restarted, Some(1_000)));
    }
}
