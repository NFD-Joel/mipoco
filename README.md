# mipoco

Minimal, performance-focused TUI terminal multiplexer for running multiple
[Claude Code](https://claude.com/claude-code) sessions side by side — with an
integrated file explorer to start a session in any folder or execute files
without leaving the terminal.

Runs inside your existing terminal (like a stripped-down Zellij) on **Linux**
and **Windows** (Windows 10 1809+, via ConPTY).

```
┌ 1:claude mipoco  2:zsh aev  3:claude dailyrecipes ─ mipoco ┐
│ ▸ projects/        │ $ claude                           │
│   ▸ aev/           │ ╭─ Claude Code ──────────────────╮ │
│   ▾ mipoco/        │ │ > fix the parser bug           │ │
│     src/           │ │ ● Working...                   │ │
│     Cargo.toml     │ ╰────────────────────────────────╯ │
│   index.html       │                                    │
└──────────────────────────────────────────────────────────┘
```

## Install

### Ubuntu / Debian — `.deb`

```sh
sudo apt install ./mipoco_<version>_amd64.deb
```

Installs `mipoco` + a launcher, and adds **mipoco** to your app menu with an
icon. Launching it from the menu opens mipoco in a terminal window (it
auto-detects alacritty, kitty, gnome-terminal, … or set `$MIPOCO_TERMINAL`).
Uninstall with `sudo apt remove mipoco`.

### Any Linux — AppImage

```sh
chmod +x mipoco-<version>-x86_64.AppImage
./mipoco-<version>-x86_64.AppImage
```

A single portable file — no install. Double-click it (or run it) and it opens
in a terminal window.

### Windows — installer

Download `mipoco-<version>-setup.exe` and run it: it installs to *Program
Files* with Start Menu + Desktop shortcuts and a clean uninstaller.

## Build from source

```sh
cargo build --release
install -Dm755 target/release/mipoco ~/.local/bin/mipoco   # or copy anywhere on PATH
```

On Windows: `cargo build --release` → `target\release\mipoco.exe`.

### Build the packages

```sh
cargo deb                               # → target/debian/mipoco_<version>_amd64.deb
bash packaging/linux/build-appimage.sh  # → target/mipoco-<version>-x86_64.AppImage
```

The Windows installer is built on Windows with NSIS — see
[`packaging/windows/README.md`](packaging/windows/README.md). App icons are
generated from `assets/mipoco.svg` by `python3 packaging/render-icons.py`.

## Keys

All mipoco commands use **Alt** so that Claude Code's own keys (Esc, Ctrl+C,
Ctrl+R, Shift+Tab, arrows, …) pass through untouched. Everything not listed
below goes verbatim to the focused terminal.

| Keys | Action |
|---|---|
| `Alt+t` | new tab |
| `Alt+q` | close focused pane — closing the last pane quits mipoco |
| `Alt+Shift+Q` | close the whole tab with all its panes |
| `Alt+1`…`Alt+9` | jump to tab |
| `Alt+,` / `Alt+.` (also `Alt+[` / `Alt+]`) | previous / next tab |
| `Alt+r` | rename tab |
| `Alt+s` / `Alt+c` | split with a shell / claude session |
| `Alt+b` | split with a claude session in **bypass mode** (`--dangerously-skip-permissions`) |
| `Alt+o` | settings overlay (changes are saved to the config file) |
| `Alt+arrows` or `Alt+h j k l` | move focus between panes |
| `Alt+Shift+arrows` | resize split |
| `Alt+z` | zoom focused pane |
| `Alt+e` | toggle / focus file explorer |
| `Alt+y` | copy mode (see below) |
| `Alt+PgUp` / `Alt+PgDn` | scrollback (any input snaps back to live) |
| `Alt+Shift+L` | passthrough mode: forward *everything*, incl. Alt keys |
| `Alt+?` | help overlay |

### Copying text

Selecting with the mouse normally grabs the whole terminal grid — explorer
panel included. mipoco therefore handles the mouse itself:

- **Drag with the mouse** inside a pane: selects only that pane's text and
  copies it to the clipboard on release. Trailing padding and any box-drawing
  frame the inner app drew (e.g. Claude's panel borders) are stripped.
- **`Alt+y` copy mode** (keyboard): `j/k` move (scrolls into scrollback at the
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
| `b` / `B` | claude in **bypass mode** (`--dangerously-skip-permissions`), as a tab / split |
| `v` | **view** the selected file in the text viewer (scrollable, inside a pane) |
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
- Extensions in `view_with_pager` (md, txt, log, json, code, …) open in the
  **text viewer inside a pane** (a horizontal split, like every other pane).
  Press `v` in the explorer to view any file this way regardless of extension.
  Scroll with `j`/`k`, arrows, `Space`/`b`, `g`/`G` or the mouse wheel; `Alt+q`
  closes the pane like any other.
  - With `viewer = "builtin"` (default) mipoco renders the text itself:
    word-wrapped (no cut-off words), with side margins and comfortable spacing.
  - With `viewer = "external"` it opens a pager in the pane instead, auto-picking
    `glow` for markdown and `bat` for code/text when installed (syntax
    highlighting), else falling back to `pager` (`less -R`).
- Anything else falls back to the OS default opener.

## Configuration

Press `Alt+o` for the in-app settings overlay — explorer-on-start, explorer
width, scrollback, shell, claude command, text viewer (builtin/external),
external pager, auto-close. Changes apply immediately and are written to the
config file.

The file lives at `~/.config/mipoco/config.toml` (Linux) or
`%APPDATA%\mipoco\config.toml` (Windows) and can also be edited by hand
(runner table and open-with-system list are file-only). All keys optional;
invalid files produce a warning, never a crash.
See [config.example.toml](config.example.toml):

```toml
# default_shell = "/bin/zsh"     # default: $SHELL (Linux), powershell (Windows)
show_explorer_on_start = false   # open the file explorer panel at startup
claude_command = "claude"        # what the c/C and Alt+c actions run
viewer = "builtin"               # text viewer: "builtin" (in-app) or "external"
pager = "less -R"                # external-mode fallback (auto-picks glow/bat)
scrollback = 5000                # lines kept per pane (primary screen only)
explorer_width = 32
auto_close_exited = false        # close panes immediately when their child exits

[runners]                        # extend/override the built-in runner table
py = "python3"
go = "go run"

# view_with_pager = ["md", "txt", "log", "json"]   # extensions opened in the pager
```

On Linux, claude and the pager run through an interactive login shell
(`$SHELL -ic`) so they're found on the PATH your shell rc sets up (e.g.
`~/.npm-global/bin` from `~/.zshrc`) — this is why claude spawns correctly even
when mipoco is launched from a desktop icon rather than a terminal.

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
