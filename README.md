# Journal

A private daily journal for Omarchy: Rust + GTK4, encrypted at rest, exportable to markdown.

## Build and install

Needs Rust, `gtk4` and GStreamer (`sudo pacman -S --needed rust gtk4 gst-plugins-good`).

```bash
cargo build --release
install -Dm755 target/release/journal ~/.local/bin/journal
install -Dm644 -t ~/.local/share/fonts/gelasio assets/fonts/*.ttf && fc-cache -f
```

The app uses Georgia when it is installed and falls back to the bundled Gelasio.

## Use

Run `journal`. The first launch sets the password; it cannot be recovered.
Press Ctrl+/ in the app for the shortcut list. `journal help` lists the
command-line and agent (MCP) commands, which work while the app is unlocked.

The vault lives in `~/.local/share/journal` (override with `JOURNAL_DIR`).

## Test

```bash
cargo test
```
