# Verifying attention notifications on Windows

This branch (`notifications`) adds desktop notifications when a Claude pane asks
for permission or finishes its turn — clicking the toast jumps to that exact
tab + pane and raises the window. Linux is verified; this is the Windows check.

> You must test the **installed** app, not `cargo run`. Windows only shows toasts
> for a *registered* AppUserModelID, and that registration happens in the
> installer.

## 1. Prerequisites (one-time)

- **Rust + MSVC toolchain** — install `rustup`, plus **Visual Studio Build Tools**
  with the "Desktop development with C++" workload (provides `link.exe`, which
  Rust on Windows needs).
- **NSIS** (for `makensis`) — `winget install NSIS.NSIS`, then add its install
  folder to PATH (or call `makensis` by full path).
- *(Optional)* **Claude Code CLI** on PATH — only needed for the "real" trigger in
  step 3. The manual trigger works without it.

## 2. Build + install (PowerShell, from the repo root)

```powershell
git checkout notifications
git pull
cargo build --release
makensis /DVERSION=0.7.1 packaging\windows\mipoco.nsi
```

Run the produced `packaging\windows\mipoco-0.7.1-setup.exe` (per-user, no admin),
then launch **mipoco from the Start-menu shortcut**.

## 3. Trigger a notification

**Easy way — no Claude Code needed.** Every mipoco pane inherits
`MIPOCO_SOCK` / `MIPOCO_TOKEN` / `MIPOCO_PANE`, so you can fire a real event from a
shell pane. Open a second pane and focus it, then in the **first** pane's shell:

```powershell
'{}' | & "$env:LOCALAPPDATA\Programs\mipoco\mipoco.exe" --hook permission
```

That pops a toast for the first pane exactly as a Claude permission prompt would.
(Use `--hook stop` to simulate "task finished".)

**Real way — if Claude Code is installed.** Open a Claude pane, give it a task that
needs a tool permission, switch focus to another pane/app, and wait.

## 4. What to check

1. A toast appears, titled **mipoco**, body like `🔐 claude — permission needed`.
2. **Settings → System → Notifications** lists **mipoco** as its own app with an
   on/off switch (the per-app OS toggle).
3. Clicking the toast brings mipoco forward and jumps to that exact tab + pane;
   the tab's `●` marker clears.

## Known caveat (Windows Terminal)

If mipoco runs inside **Windows Terminal**, the *window-raise* step may not pull
the WT tab to the foreground — WT hides the real console window behind a
pseudoconsole, so `GetConsoleWindow` can't target it. The toast, the
click → pane-switch, and the `●` marker all still work; only "raise the OS window"
is affected, and only under Windows Terminal. To test raise cleanly, run mipoco in
the **classic console** (double-clicking the exe/shortcut, not WT).

## In-app + OS controls

- Inside mipoco: **Alt+o → "desktop notifications"** toggles the whole feature
  (also removes/installs the Claude hooks in `%USERPROFILE%\.claude\settings.json`).
- OS level: the per-app switch in Windows notification settings (step 4.2).
