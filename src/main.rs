use core::fmt;
use std::process::Command;

use discord_presence::Client;
use serde::Deserialize;

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .init();

    let mut drpc = Client::new(773825528921849856);

    drpc.on_ready(|_ctx| {
        println!("READY!");
    })
    .persist();

    drpc.on_error(|ctx| {
        eprintln!("An error occured, {:?}", ctx.event);
    })
    .persist();

    drpc.start();

    loop {
        let mut buf = String::new();

        std::io::stdin().read_line(&mut buf).unwrap();
        buf.pop();

        if buf.is_empty() {
            if let Err(why) = drpc.clear_activity() {
                println!("Failed to clear presence: {}", why);
            }
        } else if let Err(why) = drpc.set_activity(|a| {
            let track = Track::from_apple_music().unwrap();
            a.state(track.to_string())
        }) {
            println!("Failed to set presence: {}", why);
        }
    }
    // drpc.block_on().unwrap();
}
#[derive(Debug, Deserialize)]
struct TrackInfo {
    trackName: String,
    artistName: String,
    albumName: String,
    duration: u32,
    playerState: String,
    message: Option<String>,
}

#[derive(Debug)]
struct Track {
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: u32,
    player_state: String,
    message: Option<String>,
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
    fn from_track_info(info: TrackInfo) -> Self {
        Self {
            track_name: info.trackName,
            artist_name: info.artistName,
            album_name: info.albumName,
            duration: info.duration,
            player_state: info.playerState,
            message: info.message,
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
        let track_info: TrackInfo = match serde_json::from_str(&output_string) {
            Ok(track_info) => track_info,
            Err(err) => {
                eprintln!("Error deserializing JSON: {}", err);
                return None;
            }
        };

        Some(Self::from_track_info(track_info))
    }
}
