# TUI

postblox-tui is a terminal UI client built with ratatui. It connects to the postblox server via REST API and WebSocket for live updates.

## Installation

```bash
# From source
cargo build --release --bin postblox-tui

# Binary is at target/release/postblox-tui
```

## Configuration

Config file location: `$XDG_CONFIG_HOME/postblox/tui.toml` (defaults to `~/.config/postblox/tui.toml`).

```toml
[server]
url = "http://localhost:3000"
api_key = "pb_abc12.deadbeef..."

[tui]
theme = "nord"
vim_mode = true
```

| Field | Env var fallback | Default | Description |
|-------|-----------------|---------|-------------|
| `server.url` | `POSTBLOX_URL` | *(required)* | postblox server URL |
| `server.api_key` | `POSTBLOX_API_KEY` | *(required)* | API key |
| `tui.theme` | — | `nord` | Color theme |
| `tui.vim_mode` | — | `true` | Enable vim keybindings |

## Themes

Four built-in themes:

| Theme | Description |
|-------|-------------|
| `nord` | Cool blue tones (default) |
| `dracula` | Dark purple accent |
| `catppuccin` | Soft pastel blues |
| `tokyo_night` / `tokyo-night` | Deep blue with bright accents |

Set via `tui.theme` in config. Unknown theme names fall back to `nord`.

## Keybindings

### Universal (all modes)

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate list items |
| `Left` / `Right` | Switch panels |
| `Tab` | Cycle panel forward |
| `Shift+Tab` | Cycle panel backward |
| `Enter` | Select / open |
| `Esc` | Back / close |
| `?` | Show help overlay |
| `Ctrl+C` | Quit (works in all modes) |

### Normal mode — Ctrl shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+N` | Compose new message |
| `Ctrl+R` | Reply to message |
| `Ctrl+F` | Open search |

### Compose mode

| Key | Action |
|-----|--------|
| `Ctrl+Enter` | Send message |
| `Esc` | Cancel compose |

### Search mode

| Key | Action |
|-----|--------|
| `Enter` | Select result |
| `Esc` | Close search |

### Help mode

Any key dismisses the help overlay.

### Vim keybindings (when `vim_mode = true`)

| Key | Action |
|-----|--------|
| `j` / `k` | Move down / up |
| `h` / `l` | Panel left / right |
| `g` / `G` | Jump to top / bottom |
| `c` | Compose |
| `r` | Reply |
| `/` | Search |
| `s` | Toggle slop filter |
| `b` | Show briefing |
| `a` | Show all inboxes |
| `q` | Quit |
| `1`-`9` | Quick jump to inbox by number |
| `y` | Approve selected (message list only) |
| `n` | Reject selected (message list only) |

## Panels

The TUI has three main panels:

- **Sidebar** — inbox list with unread counts
- **Message list** — messages for the selected inbox
- **Preview** — message content, thread view

## Live updates

The TUI connects to the WebSocket endpoint for real-time updates. Connection status is shown in the status bar with Nerd Font icons.
