# Apple Music Presence for Discord

Reports whatever Apple Music is playing on macOS to Discord via the RPC
protocol. Presents as a **Listening** activity, so Discord renders it as a music
card — album art, a seek bar, and a link back to the track — rather than as a
game with a countdown.

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

Logs land in `~/Library/Logs/apple-music-presence.<date>.log`, rotated daily
with a week kept. Run from a terminal it logs to the console instead, so an
interactive run never writes a file. `apple-music-presence.panic.log` catches
anything that dies before reaching the logging layer, and is normally empty.

Steady state is one line per track change, around 100 lines a day of heavy
listening — the rotation is there because launchd would otherwise append to a
single file forever, not because the volume is large.

To remove the agent:

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
- Paused tracks stay on screen without a progress bar, and carry a `paused`
  badge; a favourited track playing carries a `favorited` one.
- Repeated failures to read Music.app — a revoked Automation permission fails
  every single poll — are logged three times, then only once every 60 polls, so
  an agent left running cannot grow its log file without bound.
- The progress bar is anchored to the actual playhead, so it survives seeking,
  pausing and restarting the daemon.
- Discord is only sent an update when something really changed; steady playback
  produces no traffic beyond a re-assert every 30 seconds.
- Updates are only sent while the Discord socket is up. Sending into a dead one
  deadlocks permanently: the connection manager pops a queued message off the
  channel before attempting the write, drops it if the write fails, and the
  response we block on is therefore never produced — on a channel that cannot
  error, because the manager holds both of its ends. Updates are held and
  flushed on reconnect instead. If one ever does hang for over a minute, the
  process exits so the launchd agent restarts it, because nothing in-process
  can break that deadlock.
- A Discord restart is recovered from immediately. Reconnecting drops whatever
  activity was set and `Ready` does not fire twice (`READY` is a global set
  once), so the `Connected` event drives the re-push; the 30 second timer is
  only a backstop.

## Why it looks like a music player

Three things have to line up, and Discord silently falls back to the game-style
card if any is missing:

| Field | Value | Effect |
| --- | --- | --- |
| `type` | `2` (Listening) | "Listening to" instead of "Playing" |
| `timestamps` | **both** `start` and `end` | seek bar instead of a countdown |
| `status_display_type` | `2` (Details) | member list shows the track, not the app |

`details` carries the track and `state` the artist, which is the order Discord
expects; the album goes to `large_text`, which a Listening card renders as a
third line.

The `paused` and `favorited` badges are `small_image` asset keys, resolved
against the Discord application named by `APPLE_MUSIC_CLIENT_ID`. Upload art
under exactly those names in the developer portal, under Rich Presence → Art
Assets; they take a few minutes to propagate, and Discord caches application
metadata, so quit and reopen it if a badge does not show up.

## Album art

Artwork and track links come from the [iTunes Search API][itunes], which is the
same catalogue Apple Music plays from — one request per track instead of two,
no rate-limit policy to observe, and artwork that can be requested at 512px.
A song search is tried first; if it comes back empty, which happens for local
files, non-English titles and remixes, an album search stands in. iTunes labels
short releases "Album - Single", so album titles are normalised before matching.
Results are cached per track (including misses), and a failed lookup degrades to
a presence without art rather than taking the process down. When iTunes has no
match at all, the button falls back to an Apple Music search link, so there is
always something to click.

[itunes]: https://performance-partners.apple.com/search-api

## References

https://github.com/NextFire/apple-music-discord-rpc/blob/main/music-rpc.ts

## Badge artwork

`contrib/assets/` holds the two `small_image` badges as SVG source and rendered
512×512 PNG. Upload the PNGs in the developer portal under Rich Presence → Art
Assets, named exactly `paused` and `favorited`.

Discord crops `small_image` to a circle and renders it small, in the corner of
the album art, so both are a full-bleed disc with a single high-contrast glyph
and a faint inner rim — the rim is what keeps the dark pause badge from
disappearing into dark album covers. Re-render after editing the SVG with:

```bash
rsvg-convert -w 512 -h 512 contrib/assets/paused.svg -o contrib/assets/paused.png
```
