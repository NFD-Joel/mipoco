use std::sync::mpsc::Sender;

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

use crate::notify::AttentionKind;
use crate::pty::SessionId;
use crate::update::UpdateInfo;

pub enum AppEvent {
    Input(Event),
    /// Some session produced output (coalesced via the session's dirty flag);
    /// this only wakes the draw loop, so it carries no payload.
    PtyOutput,
    /// A session's reader hit EOF: the child exited.
    PtyExited(SessionId),
    /// A Claude pane needs attention (asked for permission / finished its turn),
    /// reported via its hook. Carries the pane id so we can focus it.
    Attention {
        pane: SessionId,
        kind: AttentionKind,
        message: String,
    },
    /// A notification was clicked: focus this pane and raise the window.
    FocusPane(SessionId),
    /// Startup update check finished; `Some` when a newer release exists.
    UpdateChecked(Box<UpdateInfo>),
    /// A self-update attempt finished (Ok message / Err message).
    UpdateResult(Result<String, String>),
}

pub fn spawn_input_thread(tx: Sender<AppEvent>) {
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(AppEvent::Input(ev)).is_err() {
                break;
            }
        }
    });
}

/// Translate a crossterm key event into the byte sequence a terminal would send.
/// `app_cursor` = DECCKM application cursor mode of the target screen.
pub fn encode_key(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
    let modcode = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);

    let mut buf: Vec<u8> = Vec::with_capacity(8);

    let mut cursor_seq = |fin: char| {
        if modcode == 1 {
            if app_cursor {
                buf.extend(format!("\x1bO{fin}").bytes());
            } else {
                buf.extend(format!("\x1b[{fin}").bytes());
            }
        } else {
            buf.extend(format!("\x1b[1;{modcode}{fin}").bytes());
        }
    };

    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let lower = c.to_ascii_lowercase();
                let byte = match lower {
                    'a'..='z' => lower as u8 - b'a' + 1,
                    ' ' | '@' => 0x00,
                    '[' => 0x1b,
                    '\\' => 0x1c,
                    ']' => 0x1d,
                    '^' => 0x1e,
                    '_' | '/' => 0x1f,
                    _ => return None,
                };
                if alt {
                    buf.push(0x1b);
                }
                buf.push(byte);
            } else {
                if alt {
                    buf.push(0x1b);
                }
                let mut tmp = [0u8; 4];
                buf.extend(c.encode_utf8(&mut tmp).as_bytes());
            }
        }
        KeyCode::Enter => {
            if alt {
                buf.push(0x1b);
            }
            buf.push(b'\r');
        }
        KeyCode::Backspace => {
            if alt {
                buf.push(0x1b);
            }
            buf.push(if ctrl { 0x08 } else { 0x7f });
        }
        KeyCode::Tab => buf.push(b'\t'),
        KeyCode::BackTab => buf.extend(b"\x1b[Z"),
        KeyCode::Esc => buf.push(0x1b),
        KeyCode::Up => cursor_seq('A'),
        KeyCode::Down => cursor_seq('B'),
        KeyCode::Right => cursor_seq('C'),
        KeyCode::Left => cursor_seq('D'),
        KeyCode::Home => cursor_seq('H'),
        KeyCode::End => cursor_seq('F'),
        KeyCode::Insert => buf.extend(tilde_seq(2, modcode)),
        KeyCode::Delete => buf.extend(tilde_seq(3, modcode)),
        KeyCode::PageUp => buf.extend(tilde_seq(5, modcode)),
        KeyCode::PageDown => buf.extend(tilde_seq(6, modcode)),
        KeyCode::F(n @ 1..=4) => {
            if modcode == 1 {
                let fin = [b'P', b'Q', b'R', b'S'][n as usize - 1];
                buf.extend([0x1b, b'O', fin]);
            } else {
                let fin = ['P', 'Q', 'R', 'S'][n as usize - 1];
                buf.extend(format!("\x1b[1;{modcode}{fin}").bytes());
            }
        }
        KeyCode::F(n @ 5..=12) => {
            let num = [15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5];
            buf.extend(tilde_seq(num, modcode));
        }
        _ => return None,
    }
    Some(buf)
}

/// Translate a crossterm mouse event into the xterm byte sequence the inner
/// application asked for. `col`/`row` are 0-based pane-relative cells.
/// Returns None when `mode` does not report this kind of event.
pub fn encode_mouse(
    m: &MouseEvent,
    col: u16,
    row: u16,
    mode: MouseProtocolMode,
    enc: MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    use MouseEventKind as K;
    let wanted = match mode {
        MouseProtocolMode::None => false,
        MouseProtocolMode::Press => {
            matches!(m.kind, K::Down(_) | K::ScrollUp | K::ScrollDown)
        }
        MouseProtocolMode::PressRelease => {
            matches!(m.kind, K::Down(_) | K::Up(_) | K::ScrollUp | K::ScrollDown)
        }
        MouseProtocolMode::ButtonMotion => matches!(
            m.kind,
            K::Down(_) | K::Up(_) | K::Drag(_) | K::ScrollUp | K::ScrollDown
        ),
        MouseProtocolMode::AnyMotion => true,
    };
    if !wanted {
        return None;
    }
    let btn = |b: MouseButton| match b {
        MouseButton::Left => 0u8,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut cb, release) = match m.kind {
        K::Down(b) => (btn(b), false),
        K::Up(b) => (btn(b), true),
        K::Drag(b) => (btn(b) + 32, false),
        K::Moved => (3 + 32, false),
        K::ScrollUp => (64, false),
        K::ScrollDown => (65, false),
        K::ScrollLeft => (66, false),
        K::ScrollRight => (67, false),
    };
    if m.modifiers.contains(KeyModifiers::SHIFT) {
        cb += 4;
    }
    if m.modifiers.contains(KeyModifiers::ALT) {
        cb += 8;
    }
    if m.modifiers.contains(KeyModifiers::CONTROL) {
        cb += 16;
    }
    match enc {
        MouseProtocolEncoding::Sgr => Some(
            format!(
                "\x1b[<{cb};{};{}{}",
                col + 1,
                row + 1,
                if release { 'm' } else { 'M' }
            )
            .into_bytes(),
        ),
        MouseProtocolEncoding::Default => {
            // legacy encoding: button identity is lost on release
            let cb = if release { (cb & !0b11) | 3 } else { cb };
            Some(vec![0x1b, b'[', b'M', 32 + cb, coord_byte(col), coord_byte(row)])
        }
        MouseProtocolEncoding::Utf8 => {
            let cb = if release { (cb & !0b11) | 3 } else { cb };
            let mut buf = vec![0x1b, b'[', b'M'];
            push_utf8(&mut buf, u32::from(32 + cb));
            push_utf8(&mut buf, 32 + 1 + u32::from(col));
            push_utf8(&mut buf, 32 + 1 + u32::from(row));
            Some(buf)
        }
    }
}

/// X10 coordinate: 1-based, offset 32, capped at the single-byte maximum.
fn coord_byte(v: u16) -> u8 {
    (32 + 1 + v.min(222)) as u8
}

fn push_utf8(buf: &mut Vec<u8>, v: u32) {
    if let Some(c) = char::from_u32(v) {
        let mut tmp = [0u8; 4];
        buf.extend(c.encode_utf8(&mut tmp).as_bytes());
    }
}

fn tilde_seq(num: u8, modcode: u8) -> Vec<u8> {
    if modcode == 1 {
        format!("\x1b[{num}~").into_bytes()
    } else {
        format!("\x1b[{num};{modcode}~").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn encodes_plain_and_control_chars() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('a'), KeyModifiers::NONE), false),
            Some(b"a".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            Some(vec![0x03])
        );
        assert_eq!(
            encode_key(&key(KeyCode::Char('ä'), KeyModifiers::NONE), false),
            Some("ä".as_bytes().to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Enter, KeyModifiers::NONE), false),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Backspace, KeyModifiers::NONE), false),
            Some(vec![0x7f])
        );
    }

    #[test]
    fn encodes_arrows_in_both_cursor_modes() {
        assert_eq!(
            encode_key(&key(KeyCode::Up, KeyModifiers::NONE), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Up, KeyModifiers::NONE), true),
            Some(b"\x1bOA".to_vec())
        );
        // ctrl+right uses the xterm modifier form regardless of cursor mode
        assert_eq!(
            encode_key(&key(KeyCode::Right, KeyModifiers::CONTROL), true),
            Some(b"\x1b[1;5C".to_vec())
        );
    }

    #[test]
    fn encodes_function_and_nav_keys() {
        assert_eq!(
            encode_key(&key(KeyCode::F(1), KeyModifiers::NONE), false),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::F(5), KeyModifiers::NONE), false),
            Some(b"\x1b[15~".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Delete, KeyModifiers::NONE), false),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::BackTab, KeyModifiers::SHIFT), false),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn alt_chars_get_esc_prefix() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('f'), KeyModifiers::ALT), false),
            Some(b"\x1bf".to_vec())
        );
    }

    fn mouse(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn encodes_sgr_mouse() {
        let down = mouse(MouseEventKind::Down(MouseButton::Left));
        assert_eq!(
            encode_mouse(
                &down,
                0,
                0,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr
            ),
            Some(b"\x1b[<0;1;1M".to_vec())
        );
        let up = mouse(MouseEventKind::Up(MouseButton::Left));
        assert_eq!(
            encode_mouse(
                &up,
                4,
                2,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr
            ),
            Some(b"\x1b[<0;5;3m".to_vec())
        );
    }

    #[test]
    fn encodes_x10_mouse_and_caps_coords() {
        let down = mouse(MouseEventKind::Down(MouseButton::Left));
        assert_eq!(
            encode_mouse(
                &down,
                0,
                0,
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Default
            ),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            encode_mouse(
                &down,
                500,
                500,
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Default
            ),
            Some(vec![0x1b, b'[', b'M', 32, 255, 255])
        );
    }

    #[test]
    fn mouse_mode_filters_events() {
        let drag = mouse(MouseEventKind::Drag(MouseButton::Left));
        assert_eq!(
            encode_mouse(
                &drag,
                0,
                0,
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Sgr
            ),
            None
        );
        assert!(
            encode_mouse(
                &drag,
                0,
                0,
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr
            )
            .is_some()
        );
        let wheel = mouse(MouseEventKind::ScrollUp);
        assert_eq!(
            encode_mouse(
                &wheel,
                0,
                0,
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Sgr
            ),
            Some(b"\x1b[<64;1;1M".to_vec())
        );
    }
}
