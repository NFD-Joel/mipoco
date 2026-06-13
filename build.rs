// Embed the app icon and version metadata into mipoco.exe on Windows.
// On any other host this is a no-op, so Linux/macOS builds are unaffected
// (and `cargo check --target *-windows-*` from Linux still works, since the
// build script is compiled for the build host, not the target).
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("packaging/windows/mipoco.ico");
        res.set("ProductName", "mipoco");
        res.set("FileDescription", "mipoco terminal multiplexer");
        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=icon embedding skipped: {e}");
        }
    }
}
