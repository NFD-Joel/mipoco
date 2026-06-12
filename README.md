# mipoco

Minimal, performance-focused TUI terminal multiplexer for running multiple
[Claude Code](https://claude.com/claude-code) sessions side by side — with an
integrated file explorer to start a session in any folder or execute files
without leaving the terminal.

Runs inside your existing terminal (like a stripped-down Zellij) on **Linux**
and **Windows** (Windows 10 1809+, via ConPTY).

```
┌ 1:claude  2:zsh  3:claude ────────────────────── mipoco ┐
│ ▸ projects/        │ $ claude                           │
│   ▸ aev/           │ ╭─ Claude Code ──────────────────╮ │
│   ▾ mipoco/        │ │ > fix the parser bug           │ │
│     src/           │ │ ● Working...                   │ │
│     Cargo.toml     │ ╰────────────────────────────────╯ │
│   index.html       │                                    │
└──────────────────────────────────────────────────────────┘
```

## Build & install

```sh
cargo build --release
install -Dm755 target/release/mipoco ~/.local/bin/mipoco   # or copy anywhere on PATH
```

On Windows: `cargo build --release` → `target\release\mipoco.exe`.

## Keys

All mipoco commands use **Alt** so that Claude Code's own keys (Esc, Ctrl+C,
Ctrl+R, Shift+Tab, arrows, …) pass through untouched. Everything not listed
below goes verbatim to the focused terminal.

| Keys | Action |
|---|---|
| `Alt+t` | new tab |
| `Alt+w` (or `Alt+x`) | close focused pane — the last pane closes the tab |
| `Alt+Shift+W` | close the whole tab with all its panes |
| `Alt+1`…`Alt+9` | jump to tab |
| `Alt+,` / `Alt+.` (also `Alt+[` / `Alt+]`) | previous / next tab |
| `Alt+r` | rename tab |
| `Alt+d` / `Alt+s` | split right / split down |
| `Alt+o` | settings overlay (changes are saved to the config file) |
| `Alt+arrows` or `Alt+h j k l` | move focus between panes |
| `Alt+Shift+arrows` | resize split |
| `Alt+z` | zoom focused pane |
| `Alt+e` | toggle / focus file explorer |
| `Alt+c` | copy mode (see below) |
| `Alt+PgUp` / `Alt+PgDn` | scrollback (any input snaps back to live) |
| `Alt+Shift+L` | passthrough mode: forward *everything*, incl. Alt keys |
| `Alt+?` | help overlay |
| `Alt+q` twice | quit |

### Copying text

Selecting with the mouse normally grabs the whole terminal grid — explorer
panel included. mipoco therefore handles the mouse itself:

- **Drag with the mouse** inside a pane: selects only that pane's text and
  copies it to the clipboard on release.
- **`Alt+c` copy mode** (keyboard): `j/k` move (scrolls into scrollback at the
  edges), `Space`/`v` mark, `y`/`Enter` yank, `Esc` cancel. Line-wise.
- Clipboard via `wl-copy`/`xclip`/`xsel` when available, otherwise OSC 52
  (works in most modern terminals, including over SSH).
- **`Shift+drag`** still does your terminal's native selection if you ever
  need the raw grid.

### Mouse

- Click focuses a pane (or an explorer entry; clicking the selected entry
  opens it). Scroll wheel scrolls a pane's scrollback, the explorer list, or
  sends arrow keys on alternate-screen apps.
- Applications that enable mouse reporting themselves (fzf, htop, `nano -m`,
  Claude Code, …) receive the mouse events directly, translated to their
  pane-local coordinates.

### File explorer (when focused)

| Keys | Action |
|---|---|
| `j/k` or arrows | move selection |
| `Enter` | expand/collapse dir · execute file |
| `l` / `h` | expand · collapse / jump to parent |
| `c` / `s` | new **claude** / shell tab in the selected folder |
| `C` / `S` | same, but as a split next to the current pane |
| `x` | execute the selected file |
| `.` | toggle hidden files |
| `R` | refresh |
| `Backspace` or `-` | go up: parent becomes the tree root |
| `Esc` | back to the terminal pane |

### Executing files

- Extensions in `open_with_system` (html, pdf, images, …) open with your OS
  default app (`xdg-open` / ShellExecute) — HTML lands in your browser.
- Extensions with a configured runner (`py`, `js`, `sh`, …) run **inside a new
  pane**; the pane shows `[exit: N] press Enter to close` when done.
- Anything else falls back to the OS default opener.

## Configuration

Press `Alt+o` for the in-app settings overlay — explorer-on-start, explorer
width, scrollback, shell, claude command, auto-close. Changes apply
immediately and are written to the config file.

The file lives at `~/.config/mipoco/config.toml` (Linux) or
`%APPDATA%\mipoco\config.toml` (Windows) and can also be edited by hand
(runner table and open-with-system list are file-only). All keys optional;
invalid files produce a warning, never a crash.
See [config.example.toml](config.example.toml):

```toml
# default_shell = "/bin/zsh"     # default: $SHELL (Linux), powershell (Windows)
show_explorer_on_start = false   # open the file explorer panel at startup
claude_command = "claude"        # what the explorer's c/C action runs
scrollback = 5000                # lines kept per pane (primary screen only)
explorer_width = 32
auto_close_exited = false        # close panes immediately when their child exits

[runners]                        # extend/override the built-in runner table
py = "python3"
go = "go run"
```

Note: saving from the settings overlay rewrites the file, so hand-written
comments in it are not preserved.

## Design notes

- One reader thread per PTY feeds a `vt100` parser behind a mutex; a dirty
  flag coalesces output bursts so heavy output costs one redraw, not thousands.
  Idle = zero CPU (the draw loop blocks on a channel).
- Panes are a binary split tree per tab; resizing a pane resizes the PTY
  (SIGWINCH / ConPTY) and the parser together.
- Background tabs keep parsing while hidden; the tab bar marks activity with `*`.
- If a terminal app needs Alt keys itself, use passthrough mode (`Alt+Shift+L`).
