# Building the mipoco Windows installer

Do this on a Windows PC. It produces `mipoco-<version>-setup.exe`, an installer
that puts mipoco in *Program Files*, adds Start Menu + Desktop shortcuts with the
app icon, and registers a clean uninstaller in *Add/Remove Programs*.

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
makensis /DVERSION=0.4.0 mipoco.nsi
```

Use the same version as `Cargo.toml`. The output `mipoco-0.4.0-setup.exe` lands
in `packaging\windows\`. Double-click it to install.

## Notes

- mipoco is a terminal UI. The shortcut launches `mipoco.exe`, which opens its
  own console window — that's the app. For a nicer window, run it inside
  [Windows Terminal](https://aka.ms/terminal): `wt mipoco`.
- To run `mipoco` from any shell, add the install folder
  (`C:\Program Files\mipoco`) to your `PATH`, or use the
  [Scoop](https://scoop.sh)/`cargo install` route instead.
- Prefer an **MSI**? Install [`cargo-wix`](https://github.com/volks73/cargo-wix)
  and the WiX Toolset, then `cargo wix init` + `cargo wix`. NSIS is the
  supported path here because it's self-contained and easy to customize.
