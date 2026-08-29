mod artwork;
mod player;
mod presence;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use discord_presence::Client;
use tracing::{debug, error, info, warn};

use artwork::ArtworkClient;
use player::{PlayerState, Track};

const APPLE_MUSIC_CLIENT_ID: u64 = 773825528921849856;
/// Poll rate while there is a track loaded, playing or paused.
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Poll rate while Music is closed or stopped. Each poll spawns an `osascript`
/// process, and "Music is not open" is the state a login agent spends most of
/// its life in, so it is not worth checking every second.
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How far the computed start anchor may drift before we call it a seek rather
/// than ordinary rounding between two polls.
const SEEK_TOLERANCE_SECS: u64 = 2;
/// How many consecutive poll failures are logged in full before we go quiet.
const FAILURE_LOG_LIMIT: u32 = 3;
/// Once quiet, how many further failures pass between reminders. Revoking the
/// Automation permission fails every poll forever, and a login agent writing a
/// line per poll would grow its log file without bound.
const FAILURE_LOG_EVERY: u32 = 60;
/// How often an unchanged presence is pushed again, as a backstop to the
/// `Connected` event below. Discord allows 5 activity updates per 20 seconds,
/// so this is far inside the budget.
const REASSERT_INTERVAL_SECS: u64 = 30;

/// What we last pushed to Discord, so we only speak to it when something
/// actually changed. Discord rate-limits presence updates, and steady playback
/// should produce no traffic at all.
struct Shown {
    track: Track,
    state: PlayerState,
    start: Option<u64>,
    sent_at: u64,
}

/// Whether Discord is showing something other than what is playing now.
fn presence_changed(shown: Option<&Shown>, track: &Track, start: Option<u64>) -> bool {
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

/// Whether the presence is due to be pushed again even though nothing changed.
///
/// A reconnect drops whatever activity we had set, and `Ready` does not fire a
/// second time — `READY` is a global that is only ever set once. `Connected`
/// does fire on every connect, and drives the immediate re-assert; this timer
/// is the backstop for anything that clears the activity without telling us.
fn reassert_due(shown: Option<&Shown>, now: u64) -> bool {
    shown.is_none_or(|shown| now.saturating_sub(shown.sent_at) >= REASSERT_INTERVAL_SECS)
}

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let mut drpc = Client::new(APPLE_MUSIC_CLIENT_ID);

    // Set on every connect, including reconnects, and consumed by the loop to
    // re-push the activity that Discord dropped while we were disconnected.
    let reconnected = Arc::new(AtomicBool::new(false));

    drpc.on_ready(|_ctx| info!("connected to Discord"))
        .persist();
    drpc.on_error(|ctx| error!("discord error: {:?}", ctx.event))
        .persist();
    drpc.on_connected({
        let reconnected = Arc::clone(&reconnected);
        move |_ctx| reconnected.store(true, Ordering::Relaxed)
    })
    .persist();
    drpc.on_disconnected(|_ctx| warn!("disconnected from Discord"))
        .persist();

    drpc.start();

    if let Err(err) = drpc.block_until_event(discord_presence::Event::Ready) {
        error!("never became ready: {err}");
        return;
    }

    let mut artwork = ArtworkClient::new();
    let mut shown: Option<Shown> = None;
    let mut poll_interval = ACTIVE_POLL_INTERVAL;
    let mut consecutive_failures: u32 = 0;

    loop {
        sleep(poll_interval);

        // A failed poll leaves whatever is on screen alone: it is usually
        // transient, and when it is not, tearing the presence down would not
        // help anyway.
        let track = match Track::from_apple_music() {
            Ok(track) => {
                if consecutive_failures > FAILURE_LOG_LIMIT {
                    info!("reading Music.app recovered after {consecutive_failures} failures");
                }
                consecutive_failures = 0;
                track
            }
            Err(err) => {
                consecutive_failures += 1;
                if consecutive_failures <= FAILURE_LOG_LIMIT {
                    warn!("could not read Music.app: {err}");
                    if consecutive_failures == FAILURE_LOG_LIMIT {
                        warn!("further failures will be logged every {FAILURE_LOG_EVERY} polls");
                    }
                } else if consecutive_failures.is_multiple_of(FAILURE_LOG_EVERY) {
                    warn!("still cannot read Music.app after {consecutive_failures} polls: {err}");
                }

                // Whatever is broken is unlikely to fix itself within a second.
                poll_interval = IDLE_POLL_INTERVAL;
                continue;
            }
        };

        poll_interval = if track.player_state.has_track() {
            ACTIVE_POLL_INTERVAL
        } else {
            IDLE_POLL_INTERVAL
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

        let now = presence::unix_now();
        // Only a playing track gets a progress bar; a paused one sits still.
        let stamps = match track.player_state {
            PlayerState::Playing => presence::timestamps(&track, now),
            _ => None,
        };
        let start = stamps.map(|(start, _)| start);

        let changed = presence_changed(shown.as_ref(), &track, start);
        let reconnected = reconnected.swap(false, Ordering::Relaxed);
        if !changed && !reconnected && !reassert_due(shown.as_ref(), now) {
            continue;
        }

        let art = artwork.lookup(&track);
        if changed {
            info!(
                "{:?}: {} - {}",
                track.player_state, track.artist, track.name
            );
        } else if reconnected {
            info!("reconnected, restoring presence");
        } else {
            debug!("re-asserting presence");
        }

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
            sent_at: now,
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
            favorited: false,
        }
    }

    fn shown_from(track: &Track, start: Option<u64>) -> Shown {
        Shown {
            track: track.clone(),
            state: track.player_state,
            start,
            sent_at: 0,
        }
    }

    #[test]
    fn first_track_always_updates() {
        assert!(presence_changed(None, &playing(), Some(959)));
    }

    #[test]
    fn steady_playback_is_silent() {
        let track = playing();
        let shown = shown_from(&track, Some(959));

        assert!(!presence_changed(Some(&shown), &track, Some(959)));
        // One second of rounding slop must not trigger a new update.
        assert!(!presence_changed(Some(&shown), &track, Some(960)));
    }

    #[test]
    fn seeking_updates() {
        let track = playing();
        let shown = shown_from(&track, Some(959));

        assert!(presence_changed(Some(&shown), &track, Some(800)));
    }

    #[test]
    fn pausing_updates() {
        let track = playing();
        let shown = shown_from(&track, Some(959));

        let paused = Track {
            player_state: PlayerState::Paused,
            ..track
        };
        assert!(presence_changed(Some(&shown), &paused, None));
    }

    #[test]
    fn resuming_from_pause_updates() {
        let paused = Track {
            player_state: PlayerState::Paused,
            ..playing()
        };
        let shown = shown_from(&paused, None);

        assert!(presence_changed(Some(&shown), &playing(), Some(959)));
    }

    #[test]
    fn reassert_fires_on_a_timer() {
        let track = playing();
        let mut shown = shown_from(&track, Some(959));
        shown.sent_at = 1_000;

        assert!(!reassert_due(Some(&shown), 1_000));
        assert!(!reassert_due(
            Some(&shown),
            1_000 + REASSERT_INTERVAL_SECS - 1
        ));
        assert!(reassert_due(Some(&shown), 1_000 + REASSERT_INTERVAL_SECS));
    }

    #[test]
    fn reassert_is_due_when_nothing_is_shown() {
        assert!(reassert_due(None, 0));
    }

    #[test]
    fn a_backwards_clock_does_not_wedge_the_reassert() {
        let track = playing();
        let mut shown = shown_from(&track, Some(959));
        shown.sent_at = 5_000;

        // Waking from sleep can hand us a `now` behind `sent_at`; that must not
        // underflow into a permanently-due or permanently-quiet state.
        assert!(!reassert_due(Some(&shown), 1_000));
    }

    #[test]
    fn changing_track_updates() {
        let track = playing();
        let shown = shown_from(&track, Some(959));

        let next = Track {
            name: "Cecilia".into(),
            ..track
        };
        assert!(presence_changed(Some(&shown), &next, Some(959)));
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
        assert!(presence_changed(Some(&shown), &restarted, Some(1_000)));
    }
}
