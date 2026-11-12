# Journal

A private daily journal for Omarchy: just a cursor and text. Rust + GTK4,
encrypted on disk, exportable to markdown.

## Install

Download the latest release, unpack it and run the installer. It puts the app
in `~/.local/bin`, adds the launcher entry and icon, and installs the fallback
font. Nothing needs compiling.

```bash
gh release download -R omarsimon-1/journal -p '*.tar.gz' -O - | tar -xz && journal-linux-x86_64/install.sh
```

Needs `gtk4` and GStreamer, which Omarchy already has.

### Bar button (Omarchy plugin)

This repo is also an Omarchy shell plugin that adds a bar button to open Journal:

```bash
omarchy plugin add https://github.com/omarsimon-1/journal.git --enable
```

The plugin is only the button; the app comes from the installer above.

### Keybinding

```lua
o.bind("SUPER + SHIFT + J", "Journal", "omarchy-launch-or-focus app.journal.Journal 'uwsm-app -- journal'")
```

## Use

Open Journal from the launcher. The first launch sets the password; it cannot
be recovered. Press Ctrl+/ in the app for every shortcut.

- Entries are timestamped at their first words. Ctrl+Enter starts the next one.
- Ctrl+M opens the calendar: days written, streaks, words and characters.
- Ctrl+Shift+P shows only the last few characters, for writing in public. Ctrl+[ and Ctrl+] show fewer or more.
- Ctrl+Shift+H keeps an entry private from agents.

## Agents, sync and backup

Run `journal help` for the full list.

- `journal search`, `journal read`, `journal mcp`: read-only access for agents while the app is unlocked. Private entries are never included.
- `journal summarize` and `journal ask <question>`: handed to your Omarchy default agent (`omarchy default agent`), or to a local Ollama model with `--ollama`. With a cloud agent the entries in that period leave your machine; private entries never do.
- `journal sync <host>`: two-way sync with another machine over SSH or a tailnet. Only encrypted files travel. The host is remembered and synced on every unlock and lock.
- `journal backup <folder>`: copies the encrypted vault; remembered and repeated on every lock.

The vault lives in `~/.local/share/journal` (override with `JOURNAL_DIR`).

## Build from source

```bash
cargo build --release && ./install.sh
```
