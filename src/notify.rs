//! Attention notifications: a desktop pop-up when a Claude pane needs you —
//! either it asked for a permission or it finished its turn.
//!
//! Detection rides on Claude Code's own hook events (reliable, machine-readable)
//! rather than scraping terminal text:
//!   - the `Notification` hook, matcher `permission_prompt` → "permission"
//!   - the `Stop` hook → "stop" (finished)
//!
//! Wiring: mipoco runs a loopback TCP listener ([`IpcServer`]) and injects
//! `MIPOCO_SOCK`/`MIPOCO_TOKEN`/`MIPOCO_PANE` into every pane's environment. A
//! fired hook re-invokes this same binary as `mipoco --hook <kind>`
//! ([`hook_client`]); that client reads those env vars and forwards the event —
//! tagged with the pane id — to the listener, which turns it into an
//! [`AppEvent::Attention`]. Clicking the pop-up sends [`AppEvent::FocusPane`]
//! back so the app jumps to the exact tab + pane and raises its window.
//!
//! The hook is a no-op whenever `MIPOCO_SOCK` is unset (i.e. Claude run outside
//! mipoco), so installing it globally is harmless.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::event::AppEvent;
use crate::pty::SessionId;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AttentionKind {
    /// Claude is waiting for you to approve a tool/action.
    Permission,
    /// Claude finished its turn.
    Finished,
}

impl AttentionKind {
    fn from_tag(tag: &str) -> Self {
        match tag {
            "permission" => AttentionKind::Permission,
            _ => AttentionKind::Finished,
        }
    }

    fn line(self, title: &str) -> String {
        match self {
            AttentionKind::Permission => format!("🔐 {title} — permission needed"),
            AttentionKind::Finished => format!("✅ {title} — task finished"),
        }
    }
}

/// Loopback TCP listener that turns forwarded hook events into `AppEvent`s.
/// `addr`/`token` are injected into each pane so its hooks can reach us.
pub struct IpcServer {
    pub addr: String,
    pub token: String,
}

#[derive(Deserialize)]
struct HookMsg {
    token: String,
    pane: SessionId,
    kind: String,
    #[serde(default)]
    message: String,
}

impl IpcServer {
    /// Bind `127.0.0.1:<ephemeral>` and spawn the accept loop. Returns `None`
    /// if binding fails (notifications then simply stay off).
    pub fn start(tx: Sender<AppEvent>) -> Option<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let addr = listener.local_addr().ok()?.to_string();
        let token = gen_token();
        let want = token.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = String::new();
                if stream.read_to_string(&mut buf).is_err() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<HookMsg>(buf.trim()) else {
                    continue;
                };
                if msg.token != want {
                    continue; // reject spoofed events from other local processes
                }
                let _ = tx.send(AppEvent::Attention {
                    pane: msg.pane,
                    kind: AttentionKind::from_tag(&msg.kind),
                    message: msg.message,
                });
            }
        });
        Some(Self { addr, token })
    }
}

/// `mipoco --hook <kind>`: forward a Claude hook event to the running mipoco
/// that spawned this pane. No-op (exit 0) when not running inside mipoco.
pub fn hook_client(kind: Option<String>) -> Result<()> {
    let Ok(addr) = std::env::var("MIPOCO_SOCK") else {
        return Ok(()); // not inside mipoco — silently do nothing
    };
    let token = std::env::var("MIPOCO_TOKEN").unwrap_or_default();
    let pane: SessionId = std::env::var("MIPOCO_PANE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let kind = kind.unwrap_or_else(|| "stop".into());

    // Claude delivers the hook payload as JSON on stdin; pull a human message
    // out of it when present (permission prompts carry one).
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let message = extract_message(&input);

    let payload = serde_json::json!({
        "token": token,
        "pane": pane,
        "kind": kind,
        "message": message,
    })
    .to_string();

    if let Ok(mut stream) = TcpStream::connect(&addr) {
        let _ = stream.write_all(payload.as_bytes());
        let _ = stream.flush();
        // dropping the stream closes the write half → server sees EOF
    }
    Ok(())
}

fn extract_message(input: &str) -> String {
    serde_json::from_str::<serde_json::Value>(input)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

/// The Windows AppUserModelID. Must match the one the installer registers (and
/// sets on the Start-menu shortcut) so toasts attribute to "mipoco" and appear
/// in Windows notification settings as their own app.
#[cfg(target_os = "windows")]
pub const WIN_AUMID: &str = "nfd.mipoco";

/// Show a desktop notification for `pane`. Clicking it sends
/// [`AppEvent::FocusPane`] so the app focuses the pane and raises its window —
/// wired on Linux (XDG action) and Windows (toast activation). On macOS the
/// notification shows but has no click-to-focus (use the in-app `●` marker).
pub fn show(kind: AttentionKind, pane_title: &str, message: &str, pane: SessionId, tx: Sender<AppEvent>) {
    let mut body = kind.line(pane_title);
    if !message.is_empty() {
        body.push_str(&format!("\n{message}"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use notify_rust::{Hint, Notification};
        if let Ok(handle) = Notification::new()
            .summary("mipoco")
            .body(&body)
            .icon("mipoco")
            // Advertise our desktop entry so GNOME/KDE list "mipoco" as its own
            // app in Settings → Notifications (per-app on/off, banners, sound).
            .appname("mipoco")
            .hint(Hint::DesktopEntry("mipoco".into()))
            .action("default", "Open")
            .show()
        {
            std::thread::spawn(move || {
                handle.wait_for_action(|action| {
                    if action == "default" || action == "__clicked" {
                        let _ = tx.send(AppEvent::FocusPane(pane));
                    }
                });
            });
        }
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_winrt_notification::{Duration, Toast};
        // The toast is attributed to our AUMID; clicking it focuses the pane.
        let toast = Toast::new(WIN_AUMID)
            .title("mipoco")
            .text1(&body)
            .duration(Duration::Short)
            .on_activated(move |_arg| {
                let _ = tx.send(AppEvent::FocusPane(pane));
                Ok(())
            });
        let _ = toast.show();
    }

    #[cfg(target_os = "macos")]
    {
        use notify_rust::Notification;
        let _ = Notification::new().summary("mipoco").body(&body).show();
        let _ = (pane, tx); // no click-to-focus on this backend
    }
}

// ---- Claude settings.json hook installation --------------------------------

fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

fn hook_command(tag: &str) -> Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("{} --hook {tag}", exe.to_string_lossy()))
}

/// Idempotently add mipoco's `Notification`/`Stop` hooks to
/// `~/.claude/settings.json`, preserving every other key. Safe because the hook
/// no-ops outside mipoco.
pub fn install_hooks() -> Result<()> {
    let path = settings_path().context("no home directory")?;
    let mut root = read_settings(&path)?;
    let obj = root.as_object_mut().context("settings.json is not an object")?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let hooks = hooks.as_object_mut().context("hooks is not an object")?;

    set_hook(hooks, "Notification", Some("permission_prompt"), &hook_command("permission")?);
    set_hook(hooks, "Stop", None, &hook_command("stop")?);

    write_settings(&path, &root)
}

/// Remove mipoco's hooks from `~/.claude/settings.json`, leaving foreign ones.
pub fn uninstall_hooks() -> Result<()> {
    let path = settings_path().context("no home directory")?;
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_settings(&path)?;
    if let Some(hooks) = root
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
    {
        for event in ["Notification", "Stop"] {
            if let Some(arr) = hooks.get_mut(event).and_then(|e| e.as_array_mut()) {
                arr.retain(|group| !is_mipoco_group(group));
                if arr.is_empty() {
                    hooks.remove(event);
                }
            }
        }
    }
    write_settings(&path, &root)
}

fn read_settings(path: &PathBuf) -> Result<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn write_settings(path: &PathBuf, root: &serde_json::Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(root)?)?;
    Ok(())
}

/// Replace any existing mipoco group for `event` with a fresh one. A "group" is
/// `{ "matcher"?: ..., "hooks": [ { "type": "command", "command": ... } ] }`.
fn set_hook(
    hooks: &mut serde_json::Map<String, serde_json::Value>,
    event: &str,
    matcher: Option<&str>,
    command: &str,
) {
    let entry = hooks
        .entry(event.to_string())
        .or_insert_with(|| serde_json::json!([]));
    let arr = match entry.as_array_mut() {
        Some(a) => a,
        None => {
            *entry = serde_json::json!([]);
            entry.as_array_mut().unwrap()
        }
    };
    arr.retain(|group| !is_mipoco_group(group));
    let mut group = serde_json::Map::new();
    if let Some(m) = matcher {
        group.insert("matcher".into(), serde_json::json!(m));
    }
    group.insert(
        "hooks".into(),
        serde_json::json!([{ "type": "command", "command": command }]),
    );
    arr.push(serde_json::Value::Object(group));
}

/// True when a hook group is one mipoco installed (its command runs `--hook`).
fn is_mipoco_group(group: &serde_json::Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .into_iter()
        .flatten()
        .filter_map(|h| h.get("command").and_then(|c| c.as_str()))
        .any(|c| c.contains("--hook"))
}

/// A short, hard-to-guess token to authenticate hook clients on loopback.
fn gen_token() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::process::id().hash(&mut h);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut h);
    // a couple of stack/heap addresses add a little non-determinism
    let local = 0u8;
    (&local as *const u8 as usize).hash(&mut h);
    let a = h.finish();
    // mix again for a second word
    a.hash(&mut h);
    let b = h.finish();
    format!("{a:016x}{b:016x}")
}

// ---- best-effort raise of the host terminal window -------------------------

#[cfg(target_os = "linux")]
pub mod window {
    //! X11/EWMH window activation. At startup the terminal hosting mipoco is the
    //! focused window, so we record `_NET_ACTIVE_WINDOW`; to raise, we send the
    //! WM a `_NET_ACTIVE_WINDOW` client message (honored by Mutter/GNOME).
    //! Wayland has no portable equivalent → capture yields nothing, raise no-ops.
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt, EventMask,
    };

    #[derive(Clone, Copy, Default)]
    pub struct Handle {
        win: Option<u32>,
    }

    pub fn capture() -> Handle {
        Handle {
            win: active_window().ok().flatten(),
        }
    }

    fn active_window() -> anyhow::Result<Option<u32>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let atom = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
            .reply()?
            .atom;
        let reply = conn
            .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)?
            .reply()?;
        let win = reply.value32().and_then(|mut v| v.next());
        Ok(win.filter(|&w| w != 0))
    }

    pub fn raise(handle: &Handle) {
        if let Some(win) = handle.win {
            let _ = activate(win);
        }
    }

    fn activate(win: u32) -> anyhow::Result<()> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let atom = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
            .reply()?
            .atom;
        // data: [source indication = 2 (pager), timestamp, requestor, 0, 0]
        let event = ClientMessageEvent::new(32, win, atom, [2, x11rb::CURRENT_TIME, 0, 0, 0]);
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?;
        conn.flush()?;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub mod window {
    //! Register our AppUserModelID at startup (so toasts attribute to "mipoco")
    //! and raise the console window hosting mipoco on a notification click.
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Console::GetConsoleWindow;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::Win32::UI::WindowsAndMessaging::{SW_RESTORE, SetForegroundWindow, ShowWindow};
    use windows::core::PCWSTR;

    #[derive(Clone, Copy, Default)]
    pub struct Handle {
        /// The console window handle as an integer (HWND isn't Send/Copy-stored).
        hwnd: isize,
    }

    pub fn capture() -> Handle {
        // Set the process AUMID so our toasts are grouped under "mipoco".
        let aumid: Vec<u16> = super::WIN_AUMID
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let _ = SetCurrentProcessExplicitAppUserModelID(PCWSTR(aumid.as_ptr()));
        }
        let hwnd = unsafe { GetConsoleWindow() };
        Handle {
            hwnd: hwnd.0 as isize,
        }
    }

    pub fn raise(handle: &Handle) {
        if handle.hwnd == 0 {
            return;
        }
        let hwnd = HWND(handle.hwnd as *mut core::ffi::c_void);
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub mod window {
    #[derive(Clone, Copy, Default)]
    pub struct Handle;

    pub fn capture() -> Handle {
        Handle
    }

    pub fn raise(_handle: &Handle) {
        // macOS: best-effort activate the frontmost terminal app.
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("osascript")
                .args([
                    "-e",
                    "tell application \"System Events\" to set frontmost of first process whose frontmost is true to true",
                ])
                .status();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent_and_preserves_foreign_keys() {
        let mut root = serde_json::json!({
            "permissions": { "defaultMode": "plan" },
            "hooks": {
                "Stop": [ { "hooks": [ { "type": "command", "command": "echo other" } ] } ]
            }
        });
        let obj = root.as_object_mut().unwrap();
        let hooks = obj.get_mut("hooks").unwrap().as_object_mut().unwrap();

        for _ in 0..2 {
            set_hook(hooks, "Notification", Some("permission_prompt"), "mipoco --hook permission");
            set_hook(hooks, "Stop", None, "mipoco --hook stop");
        }

        // foreign Stop hook survives; exactly one mipoco group per event
        let stop = hooks.get("Stop").unwrap().as_array().unwrap();
        assert_eq!(stop.len(), 2, "foreign + one mipoco group");
        let mipoco_stop = stop.iter().filter(|g| is_mipoco_group(g)).count();
        assert_eq!(mipoco_stop, 1);
        let notif = hooks.get("Notification").unwrap().as_array().unwrap();
        assert_eq!(notif.len(), 1);

        // foreign top-level key preserved
        assert!(root.get("permissions").is_some());
    }
}
