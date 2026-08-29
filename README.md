# Apple Music Presence for Discord

Reports whatever Apple Music is playing on macOS to Discord via the RPC
protocol, with album art, a progress bar, and a link back to the track.

## Usage

```bash
cargo run --release
```

Discord must be running first. The first launch will prompt for permission to
control Music.app (System Settings → Privacy & Security → Automation).

## Behaviour

- Polls Music.app once a second via JavaScript for Automation.
- **Never launches Music.app.** The player is only queried once `running()`
  reports true, because touching any other property would start the app.
- Presence is cleared when playback stops or Music quits.
- Paused tracks stay on screen without a progress bar.
- The progress bar is anchored to the actual playhead, so it survives seeking,
  pausing and restarting the daemon.
- Discord is only sent an update when something really changed; steady playback
  produces no traffic.

## Album art

Artwork and track links come from the [iTunes Search API][itunes], which is the
same catalogue Apple Music plays from — one request per track instead of two,
no rate-limit policy to observe, and artwork that can be requested at 512px.
Results are cached per track (including misses), and a failed lookup degrades
to a presence without art rather than taking the process down.

[itunes]: https://performance-partners.apple.com/search-api

## References

https://github.com/NextFire/apple-music-discord-rpc/blob/main/music-rpc.ts
