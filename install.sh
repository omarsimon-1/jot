#!/bin/bash
# Installs Journal for the current user: the app, its fallback font, its icon
# and its launcher entry. Run from a release folder or from a built checkout.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -x "$here/journal" ]]; then
  bin="$here/journal"
elif [[ -x "$here/target/release/journal" ]]; then
  bin="$here/target/release/journal"
else
  echo "No journal binary here. Download a release, or run: cargo build --release" >&2
  exit 1
fi
assets="$here/assets"
[[ -d "$assets" ]] || assets="$here"
fonts="$assets/fonts"
[[ -d "$fonts" ]] || fonts="$assets"

data="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 "$bin" "$HOME/.local/bin/journal"
install -Dm644 -t "$data/fonts/gelasio" "$fonts"/*.ttf
install -Dm644 "$assets/app.journal.Journal.svg" "$data/icons/hicolor/scalable/apps/app.journal.Journal.svg"
install -Dm644 "$assets/app.journal.Journal.desktop" "$data/applications/app.journal.Journal.desktop"
fc-cache -f "$data/fonts" 2>/dev/null || true
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$data/applications" 2>/dev/null || true
echo "Journal installed. Open it from the app launcher, or run: journal"
