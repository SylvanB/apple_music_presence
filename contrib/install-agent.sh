#!/bin/bash
# Installs apple_music_presence as a launchd user agent, so it starts at login.
#
# Run the binary once by hand before installing: macOS shows the Automation
# permission prompt to the foreground process that asks for it, and a launchd
# agent has no way to put that dialog in front of you.
set -euo pipefail

LABEL="com.goldsoultheory.apple-music-presence"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="$REPO/target/release/apple_music_presence"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG="$HOME/Library/Logs/apple-music-presence.log"

echo "==> Building release binary"
cargo build --release --manifest-path "$REPO/Cargo.toml"

echo "==> Writing $PLIST"
mkdir -p "$(dirname "$PLIST")" "$(dirname "$LOG")"
sed -e "s|__BINARY__|$BINARY|g" -e "s|__LOG__|$LOG|g" \
    "$REPO/contrib/$LABEL.plist" > "$PLIST"

# bootout is only an error if it was never loaded, which is fine on a first run.
echo "==> Loading agent"
launchctl bootout "gui/$UID/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$UID" "$PLIST"

echo "==> Installed. Logs: $LOG"
echo "    Stop with:      launchctl bootout gui/$UID/$LABEL"
echo "    Uninstall with: launchctl bootout gui/$UID/$LABEL && rm $PLIST"
