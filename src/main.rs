use core::fmt;
use std::collections::HashMap;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_presence::models::Activity;
use discord_presence::Client;
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use serde_json::from_str;
use uuid::Uuid;

const APPLE_MUSIC_CLIENT_ID: u64 = 773825528921849856;

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let mut drpc = Client::new(APPLE_MUSIC_CLIENT_ID);

    drpc.on_ready(|_ctx| {
        println!("READY!");
    })
    .persist();

    drpc.on_error(|ctx| {
        eprintln!("An error occured, {:?}", ctx.event);
    })
    .persist();

    drpc.start();

    _ = drpc.block_until_event(discord_presence::Event::Ready);

    let mut last_track = Track::new();
    let mut album_art_location = HashMap::<String, String>::new();
    loop {
        sleep(Duration::from_secs(1));

        let curr_track = Track::from_apple_music().unwrap();
        if last_track != curr_track {
            last_track = curr_track;
            let album_art_url = get_album_art_url(&last_track, &mut album_art_location);
            if last_track.player_state == "stopped" {
                continue;
            }
            if let Err(why) = drpc.set_activity(|a| set_activity(a, &last_track, &album_art_url)) {
                println!("Failed to set presence: {}", why);
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    id: Uuid,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    releases: Vec<Release>,
}

#[derive(Debug, Deserialize)]
struct AlbumArtQuery {
    images: Vec<ImageInfo>,
}

#[derive(Debug, Deserialize)]
struct ImageInfo {
    thumbnails: ThumbnailUrls,
}

#[derive(Debug, Deserialize)]
struct ThumbnailUrls {
    #[serde(alias = "250")]
    px250: String,
}

fn get_album_art_url(track: &Track, album_art_locations: &mut HashMap<String, String>) -> String {
    let search_query = format!(
        "http://www.musicbrainz.org/ws/2/release/?fmt=json&query=artist:{0}+recording:{1}",
        track.artist_name, track.album_name
    );
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(search_query)
        .header(USER_AGENT, "AppleDiscordPresense/1.0")
        .send()
        .unwrap();
    let mut album_art_url = String::new();
    if response.status().is_success() {
        // Get the response text
        let body = response.text().unwrap();
        // println!("Response: {}", body);
        let search_results: SearchQuery = serde_json::from_str(&body).unwrap();
        let album_art_query = format!(
            "https://coverartarchive.org/release/{0:?}/",
            search_results.releases[0].id
        );

        let album_art_search_response = client
            .get(album_art_query)
            .header(USER_AGENT, "AppleDiscordPresense/1.0")
            .send()
            .unwrap();

        if album_art_search_response.status().is_success() {
            let body = album_art_search_response.text().unwrap();
            let album_art_results: AlbumArtQuery = serde_json::from_str(&body).unwrap();
            album_art_url = album_art_results.images[0].thumbnails.px250.clone();
        }
    }

    album_art_url
}

fn set_activity(a: Activity, track: &Track, album_art_url: &str) -> Activity {
    // let url = format!(
    //     "https://www.youtube.com/results?search_query={0} {1}",
    //     &track.artist_name, &track.track_name
    // );

    // println!("Youtube URL: {}", url);
    a.assets(|ass| {
        ass.large_text(&track.artist_name)
            .large_text(&track.album_name)
            .large_image(album_art_url)
    })
    .timestamps(|ts| {
        ts.start(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
    })
    .state(track.to_string())
}

#[derive(Debug, Deserialize)]
struct Track {
    #[serde(default, alias = "trackName")]
    track_name: String,
    #[serde(default, alias = "artistName")]
    artist_name: String,
    #[serde(default, alias = "albumName")]
    album_name: String,
    #[serde(default)]
    duration: f64,
    #[serde(default, alias = "playerState")]
    player_state: String,
    message: Option<String>,
}

impl PartialEq for Track {
    fn eq(&self, other: &Self) -> bool {
        self.track_name == other.track_name
            && self.artist_name == other.artist_name
            && self.album_name == other.album_name
    }
}

impl fmt::Display for Track {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.track_name, self.artist_name)?;
        // write!(f, "Album Name: {}\n", self.album_name)?;
        // write!(f, "Duration: {} seconds\n", self.duration)?;
        // write!(f, "Player State: {}\n", self.player_state)?;

        // if let Some(message) = &self.message {
        //     write!(f, "Message: {}\n", message)?;
        // }

        Ok(())
    }
}

impl Track {
    fn new() -> Self {
        Self {
            track_name: String::new(),
            artist_name: String::new(),
            album_name: String::new(),
            duration: 0.0,
            player_state: String::new(),
            message: None,
        }
    }

    fn from_apple_music() -> Option<Self> {
        let output = match Command::new("osascript")
            .arg("-l")
            .arg("JavaScript")
            .arg("-e")
            .arg(
                r#"
                    const Music = Application("Music");
                    const output = {};
                    if (Music.playerState() === "playing") {
                      const track = Music.currentTrack;
                      output.trackName = track.name();
                      output.artistName = track.artist();
                      output.albumName = track.album();
                      output.duration = track.duration();
                      output.playerState = Music.playerState();
                    } else {
                      output.playerState = "stopped";
                      output.message = "No track is currently playing.";
                    }
                    JSON.stringify(output)
                "#,
            )
            .output()
        {
            Ok(output) => output,
            Err(err) => {
                eprintln!("Error executing osascript: {}", err);
                return None;
            }
        };

        if !output.status.success() {
            eprintln!("Command execution failed: {:?}", output);
            return None;
        }

        let output_string = String::from_utf8_lossy(&output.stdout);
        match serde_json::from_str(&output_string) {
            Ok(track_info) => Some(track_info),
            Err(err) => {
                eprintln!("Error deserializing JSON: {}", err);
                None
            }
        }
    }
}
