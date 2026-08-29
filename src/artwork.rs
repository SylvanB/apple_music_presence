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
    /// Album searches carry no track URL, only a collection one.
    #[serde(default, rename = "collectionViewUrl")]
    collection_view_url: Option<String>,
}

impl SearchResult {
    fn to_artwork(&self) -> Option<Artwork> {
        Some(Artwork {
            image_url: upscale(self.artwork_url_100.as_deref()?),
            track_url: self
                .track_view_url
                .clone()
                .or_else(|| self.collection_view_url.clone()),
        })
    }
}

/// iTunes serves 100px thumbnails by default; the same path happily serves
/// larger renditions, and Discord renders the bigger asset far more cleanly.
fn upscale(artwork_url: &str) -> String {
    artwork_url.replace("100x100bb", "512x512bb")
}

/// iTunes labels short releases "Album - Single" / "Album - EP" where Music.app
/// reports the bare title, so both sides are stripped before comparison.
fn normalise_album(album: &str) -> String {
    let album = album.trim();
    let album = album
        .strip_suffix(" - Single")
        .or_else(|| album.strip_suffix(" - EP"))
        .unwrap_or(album);

    album.to_lowercase()
}

/// Picks the result whose album matches the one actually playing, since a
/// search for "artist track" routinely returns the single, a live cut and
/// three compilations before the album the listener is on.
fn pick_best<'a>(results: &'a [SearchResult], album: &str) -> Option<&'a SearchResult> {
    let wanted = normalise_album(album);

    results
        .iter()
        .find(|result| {
            result
                .collection_name
                .as_deref()
                .is_some_and(|name| normalise_album(name) == wanted)
        })
        .or_else(|| results.first())
}

/// A "search Apple Music for this" link, for tracks iTunes could not match.
/// Always available, so the presence never ends up without a button.
pub fn search_url(track: &Track) -> Option<String> {
    let term = format!("{} {}", track.artist, track.name);
    let term = term.trim();
    if term.is_empty() {
        return None;
    }

    reqwest::Url::parse_with_params("https://music.apple.com/search", &[("term", term)])
        .ok()
        .map(String::from)
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
        if !track.name.is_empty() {
            let term = format!("{} {}", track.artist, track.name);
            if let Some(artwork) = self.search_entity(&term, "song", &track.album)? {
                return Ok(Some(artwork));
            }
        }

        // Local files, non-English titles and remixes routinely miss the song
        // index but still match at the album level, which is all the art needs.
        if track.album.is_empty() {
            return Ok(None);
        }

        let term = format!("{} {}", track.artist, track.album);
        self.search_entity(&term, "album", &track.album)
    }

    fn search_entity(
        &self,
        term: &str,
        entity: &str,
        album: &str,
    ) -> Result<Option<Artwork>, reqwest::Error> {
        let response = self
            .http
            .get(SEARCH_ENDPOINT)
            .query(&[("term", term), ("entity", entity), ("limit", SEARCH_LIMIT)])
            .send()?
            .error_for_status()?
            .json::<SearchResponse>()?;

        Ok(pick_best(&response.results, album).and_then(SearchResult::to_artwork))
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

    fn album_fixture() -> SearchResponse {
        serde_json::from_str(include_str!("../tests/fixtures/itunes_album_search.json")).unwrap()
    }

    /// Hits the live iTunes API, so it is not part of the default run:
    /// `cargo test -- --ignored`. Worth having, since the one thing unit tests
    /// cannot catch is Apple changing the response shape underneath us.
    #[test]
    #[ignore]
    fn live_lookup_finds_art_by_song_and_by_album() {
        let mut client = ArtworkClient::new();

        let known = Track {
            artist: "Simon & Garfunkel".into(),
            name: "The Boxer".into(),
            album: "Bridge Over Troubled Water".into(),
            ..Track::default()
        };
        let art = client.lookup(&known).expect("song search should find art");
        assert!(art.image_url.contains("512x512bb"));
        assert!(art.track_url.is_some());

        // No such song, but the album is real: only the fallback can serve this.
        let album_only = Track {
            artist: "Simon & Garfunkel".into(),
            name: String::new(),
            album: "Bridge Over Troubled Water".into(),
            ..Track::default()
        };
        let art = client
            .lookup(&album_only)
            .expect("album search should find art");
        assert!(art.image_url.contains("512x512bb"));

        // Nonsense must degrade to no art rather than panicking.
        let nonsense = Track {
            artist: "zzqqxx".into(),
            name: "zzqqxx".into(),
            album: "zzqqxx".into(),
            ..Track::default()
        };
        assert!(client.lookup(&nonsense).is_none());
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
    fn matches_across_itunes_single_and_ep_suffixes() {
        // Music.app reports "I Don't Wanna Change"; iTunes calls the same
        // release "I Don't Wanna Change - Single".
        let response = album_fixture();
        let picked = pick_best(&response.results, "I Don't Wanna Change").unwrap();

        assert_eq!(
            picked.collection_name.as_deref(),
            Some("I Don't Wanna Change - Single")
        );
        assert_eq!(normalise_album("Abbey Road - EP"), "abbey road");
    }

    #[test]
    fn album_results_fall_back_to_the_collection_link() {
        let response = album_fixture();
        let artwork = pick_best(&response.results, "I Don't Wanna Change")
            .and_then(SearchResult::to_artwork)
            .expect("artwork");

        assert!(artwork.image_url.contains("512x512bb"));
        // Album hits carry no trackViewUrl, so the collection URL stands in.
        assert!(artwork
            .track_url
            .as_deref()
            .is_some_and(|url| url.contains("/album/i-dont-wanna-change-single/")));
    }

    #[test]
    fn a_result_without_art_yields_nothing() {
        let bare = SearchResult {
            artwork_url_100: None,
            collection_name: Some("Whatever".into()),
            track_view_url: Some("https://music.apple.com/x".into()),
            collection_view_url: None,
        };

        assert!(bare.to_artwork().is_none());
    }

    #[test]
    fn song_hits_prefer_the_track_link_over_the_collection_link() {
        let response = fixture();
        let artwork = pick_best(&response.results, "Bridge Over Troubled Water")
            .and_then(SearchResult::to_artwork)
            .expect("artwork");

        assert!(artwork
            .track_url
            .as_deref()
            .is_some_and(|url| url.contains("?i=")));
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
