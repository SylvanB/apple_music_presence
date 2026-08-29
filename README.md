# Apple Music Presence for Discord

Reports whatever Apple Music is playing on macOS to Discord via the RPC
protocol, with album art, a progress bar, and a link back to the track.

## Usage

```bash
cargo run --release
```

Discord must be running first. The first launch will prompt for permission to
control Music.app (System Settings → Privacy & Security → Automation).

## Running at login

```bash
cargo run --release            # once, to grant the Automation prompt
./contrib/install-agent.sh     # then install the launchd agent
```

Do the manual run first. macOS shows the Automation permission dialog to the
foreground process that triggers it, and a launchd agent has no way to put that
dialog in front of you — installed first, it would simply fail to read Music.

Logs land in `~/Library/Logs/apple-music-presence.log`. To remove it:

```bash
launchctl bootout gui/$UID/com.goldsoultheory.apple-music-presence
rm ~/Library/LaunchAgents/com.goldsoultheory.apple-music-presence.plist
```

## Behaviour

- Polls Music.app via JavaScript for Automation: once a second while a track is
  loaded, every five seconds while Music is closed or stopped.
- **Never launches Music.app.** The player is only queried once `running()`
  reports true, because touching any other property would start the app.
- Presence is cleared when playback stops or Music quits.
- Paused tracks stay on screen without a progress bar.
- The progress bar is anchored to the actual playhead, so it survives seeking,
  pausing and restarting the daemon.
- Discord is only sent an update when something really changed; steady playback
  produces no traffic beyond a re-assert every 30 seconds.
- The re-assert is what survives a Discord restart. `discord-presence` exposes
  no reconnect signal — `READY` is a global set once, and the `Ready` event is
  suppressed on the second connect — so a restart silently drops the activity
  and only a timer brings it back.

## Album art

Artwork and track links come from the [iTunes Search API][itunes], which is the
same catalogue Apple Music plays from — one request per track instead of two,
no rate-limit policy to observe, and artwork that can be requested at 512px.
A song search is tried first; if it comes back empty, which happens for local
files, non-English titles and remixes, an album search stands in. iTunes labels
short releases "Album - Single", so album titles are normalised before matching.
Results are cached per track (including misses), and a failed lookup degrades to
a presence without art rather than taking the process down.

[itunes]: https://performance-partners.apple.com/search-api

## References

https://github.com/NextFire/apple-music-discord-rpc/blob/main/music-rpc.ts
