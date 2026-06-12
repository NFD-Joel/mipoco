use std::io::Write;

/// Copy text to the system clipboard. External tools are tried first because
/// their exit status is observable; OSC 52 is the fallback — most modern
/// terminals honor it (also over SSH) but it cannot report failure.
/// Returns the name of the mechanism used.
pub fn copy(text: &str) -> &'static str {
    #[cfg(unix)]
    {
        let tools: &[(&str, &[&str], &str)] = &[
            ("wl-copy", &[], "WAYLAND_DISPLAY"),
            ("xclip", &["-selection", "clipboard"], "DISPLAY"),
            ("xsel", &["--clipboard", "--input"], "DISPLAY"),
        ];
        for (prog, args, env) in tools {
            if std::env::var_os(env).is_none() {
                continue;
            }
            if pipe_to(prog, args, text) {
                return prog;
            }
        }
    }
    osc52(text);
    "OSC 52"
}

#[cfg(unix)]
fn pipe_to(prog: &str, args: &[&str], text: &str) -> bool {
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(text.as_bytes()).is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }
    child.wait().is_ok_and(|s| s.success())
}

/// OSC 52: ask the outer terminal emulator to set the clipboard.
fn osc52(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let _ = out.flush();
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(T[(n >> shift) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
