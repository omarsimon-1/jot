#!/bin/bash
# Installs Jot for the current user: the app, its fallback font, its icon and
# its launcher entry. Run from a release folder or from a built checkout.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -x "$here/jot" ]]; then
  bin="$here/jot"
elif [[ -x "$here/target/release/jot" ]]; then
  bin="$here/target/release/jot"
else
  echo "No jot binary here. Download a release, or run: cargo build --release" >&2
  exit 1
fi
assets="$here/assets"
[[ -d "$assets" ]] || assets="$here"
fonts="$assets/fonts"
[[ -d "$fonts" ]] || fonts="$assets"

data="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 "$bin" "$HOME/.local/bin/jot"
install -Dm644 -t "$data/fonts/gelasio" "$fonts"/*.ttf
install -Dm644 "$assets/app.jot.Jot.svg" "$data/icons/hicolor/scalable/apps/app.jot.Jot.svg"
install -Dm644 "$assets/app.jot.Jot.desktop" "$data/applications/app.jot.Jot.desktop"

fc-cache -f "$data/fonts" 2>/dev/null || true
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$data/applications" 2>/dev/null || true
echo "Jot installed. Open it from the app launcher, or run: jot"
