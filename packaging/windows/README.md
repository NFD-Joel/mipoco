# Building the mipoco Windows installer

The installer `mipoco-<version>-setup.exe` is a **per-user** install: it goes to
`%LOCALAPPDATA%\Programs\mipoco` (no admin / no UAC prompt), adds Start Menu +
Desktop shortcuts with the app icon, and registers a clean uninstaller in
*Add/Remove Programs*. Because that folder is user-writable, the in-app updater
(`Alt+u`) can self-replace the binary without elevation.

## Easiest: let CI build it for you

You don't need a Windows PC. The GitHub Actions workflow
[`.github/workflows/release.yml`](../../.github/workflows/release.yml) builds the
Windows installer + updater `.zip` (and the Linux `.deb` + `.tar.gz`) on a
Windows runner and attaches them to a GitHub Release. Just push a version tag:

```bash
git tag v0.7.1 && git push origin v0.7.1
```

The rest of this file is for building the installer **manually on a Windows PC**.

## 1. One-time setup

- **Rust (MSVC toolchain)** — install from <https://rustup.rs>. Accept the
  default `x86_64-pc-windows-msvc` toolchain. This also pulls the MSVC build
  tools; the Windows SDK it installs provides `rc.exe`, which `build.rs` uses to
  embed the icon into `mipoco.exe`. (If `rc.exe` is missing the build still
  succeeds — it just skips the embedded icon; the installer/shortcut icon below
  still works.)
- **NSIS** — install from <https://nsis.sourceforge.io> or:
  ```powershell
  winget install NSIS.NSIS
  ```
  Make sure `makensis.exe` is on your `PATH` (it's in
  `C:\Program Files (x86)\NSIS\`).

## 2. Build the binary

From the repo root:

```powershell
cargo build --release
```

This creates `target\release\mipoco.exe` with the icon and version metadata
baked in.

## 3. Build the installer

```powershell
cd packaging\windows
makensis /DVERSION=0.7.1 mipoco.nsi
```

Use the same version as `Cargo.toml`. The output `mipoco-0.7.1-setup.exe` lands
in `packaging\windows\`. Double-click it to install.

## 4. (Optional) Asset for the in-app updater

mipoco's `Alt+u` self-update downloads a `.zip` from the GitHub Release and
swaps its own binary. To support Windows users, upload an archive whose name
contains `x86_64` + `windows`, with `mipoco.exe` inside:

```powershell
# from the repo root, after `cargo build --release`
Compress-Archive -Path target\release\mipoco.exe `
  -DestinationPath mipoco-x86_64-pc-windows-msvc.zip
```

Attach that `.zip` to the same GitHub Release as the Linux
`mipoco-x86_64-unknown-linux-gnu.tar.gz`. (The CI workflow above does this
automatically.) Since the installer is per-user, `Alt+u` updates apply without
admin rights.

## Notes

- mipoco is a terminal UI. The shortcut launches `mipoco.exe`, which opens its
  own console window — that's the app. For a nicer window, run it inside
  [Windows Terminal](https://aka.ms/terminal): `wt mipoco`.
- To run `mipoco` from any shell, add the install folder
  (`%LOCALAPPDATA%\Programs\mipoco`) to your `PATH`, or use the
  [Scoop](https://scoop.sh)/`cargo install` route instead.
- Prefer an **MSI**? Install [`cargo-wix`](https://github.com/volks73/cargo-wix)
  and the WiX Toolset, then `cargo wix init` + `cargo wix`. NSIS is the
  supported path here because it's self-contained and easy to customize.
