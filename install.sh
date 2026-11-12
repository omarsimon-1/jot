#!/bin/bash
# Installs Verso for the current user: the app, its fallback font, its icon and
# its launcher entry. Run from a release folder or from a built checkout.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -x "$here/verso" ]]; then
  bin="$here/verso"
elif [[ -x "$here/target/release/verso" ]]; then
  bin="$here/target/release/verso"
else
  echo "No verso binary here. Download a release, or run: cargo build --release" >&2
  exit 1
fi
assets="$here/assets"
[[ -d "$assets" ]] || assets="$here"
fonts="$assets/fonts"
[[ -d "$fonts" ]] || fonts="$assets"

data="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 "$bin" "$HOME/.local/bin/verso"
install -Dm644 -t "$data/fonts/gelasio" "$fonts"/*.ttf
install -Dm644 "$assets/app.verso.Verso.svg" "$data/icons/hicolor/scalable/apps/app.verso.Verso.svg"
install -Dm644 "$assets/app.verso.Verso.desktop" "$data/applications/app.verso.Verso.desktop"

# Replace an install made under this app's earlier name.
if [[ -f "$data/applications/app.jot.Jot.desktop" ]]; then
  rm -f -- "$HOME/.local/bin/jot" "$data/applications/app.jot.Jot.desktop" \
    "$data/icons/hicolor/scalable/apps/app.jot.Jot.svg"
fi

fc-cache -f "$data/fonts" 2>/dev/null || true
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$data/applications" 2>/dev/null || true
echo "Verso installed. Open it from the app launcher, or run: verso"
