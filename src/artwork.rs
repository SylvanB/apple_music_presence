//! Album art and track links, sourced from the iTunes Search API.
//!
//! iTunes is used in preference to MusicBrainz/Cover Art Archive because it is
//! the same catalogue Apple Music plays from: one request instead of two, no
//! rate-limit policy to observe, artwork that is actually resizable, and a
//! canonical Apple Music URL we can hang a button off.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use tracing::{debug, warn};

use crate::player::Track;

const SEARCH_ENDPOINT: &str = "https://itunes.apple.com/search";
const USER_AGENT: &str = concat!("apple_music_presence/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Enough candidates to find the right album without paying for a big response.
const SEARCH_LIMIT: &str = "5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    pub image_url: String,
    /// Canonical Apple Music link for the track, when iTunes supplies one.
    pub track_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    #[serde(default, rename = "artworkUrl100")]
    artwork_url_100: Option<String>,
    #[serde(default, rename = "collectionName")]
    collection_name: Option<String>,
    #[serde(default, rename = "trackViewUrl")]
    track_view_url: Option<String>,
}

/// iTunes serves 100px thumbnails by default; the same path happily serves
/// larger renditions, and Discord renders the bigger asset far more cleanly.
fn upscale(artwork_url: &str) -> String {
    artwork_url.replace("100x100bb", "512x512bb")
}

/// Picks the result whose album matches the one actually playing, since a
/// search for "artist track" routinely returns the single, a live cut and
/// three compilations before the album the listener is on.
fn pick_best<'a>(results: &'a [SearchResult], album: &str) -> Option<&'a SearchResult> {
    results
        .iter()
        .find(|result| {
            result
                .collection_name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(album))
        })
        .or_else(|| results.first())
}

pub struct ArtworkClient {
    http: reqwest::blocking::Client,
    /// Negative results are cached too, so an unfindable track costs one
    /// request rather than one per poll.
    cache: HashMap<String, Option<Artwork>>,
}

impl ArtworkClient {
    pub fn new() -> Self {
        let http = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            cache: HashMap::new(),
        }
    }

    pub fn lookup(&mut self, track: &Track) -> Option<Artwork> {
        if track.name.is_empty() && track.album.is_empty() {
            return None;
        }

        let key = format!("{}\u{1}{}\u{1}{}", track.artist, track.album, track.name);
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }

        let artwork = match self.search(track) {
            Ok(artwork) => artwork,
            // A lookup failure must never take the presence down with it; we
            // just show the track without art and retry on the next change.
            Err(err) => {
                warn!(
                    "artwork lookup failed for {} - {}: {err}",
                    track.artist, track.name
                );
                None
            }
        };

        debug!("artwork for {} - {}: {artwork:?}", track.artist, track.name);
        self.cache.insert(key, artwork.clone());
        artwork
    }

    fn search(&self, track: &Track) -> Result<Option<Artwork>, reqwest::Error> {
        let term = format!("{} {}", track.artist, track.name);
        let response = self
            .http
            .get(SEARCH_ENDPOINT)
            .query(&[
                ("term", term.as_str()),
                ("entity", "song"),
                ("limit", SEARCH_LIMIT),
            ])
            .send()?
            .error_for_status()?
            .json::<SearchResponse>()?;

        Ok(
            pick_best(&response.results, &track.album).and_then(|result| {
                result.artwork_url_100.as_deref().map(|url| Artwork {
                    image_url: upscale(url),
                    track_url: result.track_view_url.clone(),
                })
            }),
        )
    }
}

impl Default for ArtworkClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> SearchResponse {
        serde_json::from_str(include_str!("../tests/fixtures/itunes_search.json")).unwrap()
    }

    #[test]
    fn upscales_artwork_urls() {
        assert_eq!(
            upscale("https://is1-ssl.mzstatic.com/image/thumb/abc/100x100bb.jpg"),
            "https://is1-ssl.mzstatic.com/image/thumb/abc/512x512bb.jpg"
        );
    }

    #[test]
    fn upscale_leaves_unexpected_urls_alone() {
        assert_eq!(
            upscale("https://example.invalid/art.png"),
            "https://example.invalid/art.png"
        );
    }

    #[test]
    fn prefers_the_album_that_is_actually_playing() {
        let response = fixture();
        // The fixture's first hit is "Bridge Over Troubled Water"; matching on
        // the album must reach the compilation further down the list.
        let picked = pick_best(&response.results, "The Best of Simon & Garfunkel").unwrap();

        assert_eq!(
            picked.collection_name.as_deref(),
            Some("The Best of Simon & Garfunkel")
        );
    }

    #[test]
    fn album_match_is_case_insensitive() {
        let response = fixture();
        let picked = pick_best(&response.results, "bridge over troubled water").unwrap();

        assert_eq!(
            picked.collection_name.as_deref(),
            Some("Bridge Over Troubled Water")
        );
    }

    #[test]
    fn falls_back_to_the_first_result() {
        let response = fixture();
        let picked = pick_best(&response.results, "An Album That Does Not Exist").unwrap();

        assert_eq!(
            picked.collection_name.as_deref(),
            Some("Bridge Over Troubled Water")
        );
    }

    #[test]
    fn no_results_yields_no_artwork() {
        assert!(pick_best(&[], "Anything").is_none());
    }

    #[test]
    fn fixture_results_carry_art_and_a_track_link() {
        let response = fixture();
        let picked = pick_best(&response.results, "Bridge Over Troubled Water").unwrap();

        assert!(picked.artwork_url_100.is_some());
        assert!(picked
            .track_view_url
            .as_deref()
            .is_some_and(|url| url.starts_with("https://music.apple.com/")));
    }
}
