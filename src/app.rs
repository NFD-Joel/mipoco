use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use portable_pty::CommandBuilder;
use ratatui::layout::{Margin, Position, Rect};

use crate::clipboard;
use crate::config::{Config, ViewerMode};
use crate::event::{AppEvent, encode_key, encode_mouse};
use crate::exec::{self, ExecOutcome};
use crate::explorer::Explorer;
use crate::layout::{NavDir, PaneNode, SplitDir, Tab, directional_focus};
use crate::notify::{self, AttentionKind, IpcServer};
use crate::persist::{self, PaneKind};
use crate::pty::{PtySession, SessionId};
use crate::setup::{Step, Wizard};
use crate::update::{self, UpdateInfo};
use crate::viewer::{Viewer, WrapMode};

/// Direction every split uses (side by side; vertical divider).
const SPLIT_DIR: SplitDir = SplitDir::Horizontal;

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Focus {
    Pane,
    Explorer,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum SettingKind {
    Bool,
    /// A fixed set of values cycled with Enter/Space (e.g. viewer mode).
    Choice,
    Number,
    Text,
    /// An action triggered by Enter rather than a stored value (e.g. re-run setup).
    Action,
}

pub struct SettingDef {
    pub label: &'static str,
    pub kind: SettingKind,
}

pub const SETTINGS: &[SettingDef] = &[
    SettingDef {
        label: "show explorer on start",
        kind: SettingKind::Bool,
    },
    SettingDef {
        label: "auto-close exited panes",
        kind: SettingKind::Bool,
    },
    SettingDef {
        label: "explorer width",
        kind: SettingKind::Number,
    },
    SettingDef {
        label: "scrollback lines",
        kind: SettingKind::Number,
    },
    SettingDef {
        label: "default shell",
        kind: SettingKind::Text,
    },
    SettingDef {
        label: "claude command",
        kind: SettingKind::Text,
    },
    SettingDef {
        label: "text viewer",
        kind: SettingKind::Choice,
    },
    SettingDef {
        label: "external pager",
        kind: SettingKind::Text,
    },
    SettingDef {
        label: "check for updates",
        kind: SettingKind::Bool,
    },
    SettingDef {
        label: "desktop notifications",
        kind: SettingKind::Bool,
    },
    SettingDef {
        label: "restore last session",
        kind: SettingKind::Bool,
    },
    SettingDef {
        label: "re-run setup wizard",
        kind: SettingKind::Action,
    },
];

/// Which view the update overlay is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UpdateMode {
    /// Upgrade / changelog / dismiss prompt.
    Prompt,
    /// Scrollable release notes.
    Changelog,
}

/// State of the update overlay (opened with Alt+u when an update is available).
pub struct UpdateOverlay {
    pub mode: UpdateMode,
    pub scroll: u16,
}

/// Keyboard copy mode: line-wise selection on the focused pane's screen.
pub struct CopyMode {
    pub cursor_row: u16,
    pub anchor_row: Option<u16>,
}

impl CopyMode {
    /// Selected row range (inclusive); just the cursor row before anchoring.
    pub fn range(&self) -> (u16, u16) {
        match self.anchor_row {
            Some(a) => (a.min(self.cursor_row), a.max(self.cursor_row)),
            None => (self.cursor_row, self.cursor_row),
        }
    }
}

/// In-progress mouse drag selection inside one pane (pane-relative cells).
pub struct MouseSel {
    pub id: SessionId,
    pub start: (u16, u16), // (row, col)
    pub end: (u16, u16),
    pub dragged: bool,
}

impl MouseSel {
    /// Endpoints in reading order ((row, col) tuples compare lexicographically).
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

pub struct App {
    pub config: Config,
    pub sessions: HashMap<SessionId, PtySession>,
    /// Built-in viewer panes, keyed by the same id space as `sessions`.
    pub viewers: HashMap<SessionId, Viewer>,
    /// How each restorable pane was created (shell / claude / builtin viewer),
    /// keyed by session id. Transient panes (runner/pager/external viewer) are
    /// absent. Drives the session-restore snapshot.
    pub session_meta: HashMap<SessionId, PaneKind>,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub focus: Focus,
    pub explorer: Explorer,
    pub explorer_visible: bool,
    pub explorer_rect: Rect,
    /// Outer rects of the active tab's panes, recomputed each frame.
    pub pane_rects: Vec<(SessionId, Rect)>,
    /// Whether panes are drawn with borders this frame (more than one pane).
    pub bordered: bool,
    pub passthrough: bool,
    pub show_help: bool,
    pub settings_open: bool,
    pub settings_sel: usize,
    pub settings_edit: Option<String>,
    pub copy_mode: Option<CopyMode>,
    pub mouse_sel: Option<MouseSel>,
    pub renaming: Option<String>,
    /// First-run setup wizard, when active (modal — captures all input).
    pub wizard: Option<Wizard>,
    /// A newer release found by the startup check, if any.
    pub update: Option<UpdateInfo>,
    /// The update overlay (Alt+u), when open.
    pub update_overlay: Option<UpdateOverlay>,
    /// A self-update is downloading/applying right now.
    pub updating: bool,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    /// Panes that have signalled they need attention (permission / finished);
    /// drives the tab-bar dot and pane border, cleared when the pane is focused.
    pub attention: HashSet<SessionId>,
    next_id: SessionId,
    tx: Sender<AppEvent>,
    /// Loopback listener + token for Claude hooks; `None` if it failed to bind.
    ipc: Option<IpcServer>,
    /// Host terminal window, captured at startup, raised on notification click.
    win: notify::window::Handle,
    term_size: (u16, u16),
}

impl App {
    pub fn new(config: Config, tx: Sender<AppEvent>, term_size: (u16, u16)) -> Result<Self> {
        let explorer_visible = config.show_explorer_on_start;
        let explorer = Explorer::new(config.explorer_roots.clone());
        // Bind the hook listener (best-effort) and remember the host terminal
        // window before any pane is spawned so panes inherit the right env.
        let ipc = config.notifications.then(|| IpcServer::start(tx.clone())).flatten();
        let win = notify::window::capture();
        let mut app = Self {
            config,
            sessions: HashMap::new(),
            viewers: HashMap::new(),
            session_meta: HashMap::new(),
            tabs: Vec::new(),
            active_tab: 0,
            focus: Focus::Pane,
            explorer,
            explorer_visible,
            explorer_rect: Rect::default(),
            pane_rects: Vec::new(),
            bordered: false,
            passthrough: false,
            show_help: false,
            settings_open: false,
            settings_sel: 0,
            settings_edit: None,
            copy_mode: None,
            mouse_sel: None,
            renaming: None,
            wizard: None,
            update: None,
            update_overlay: None,
            updating: false,
            status_msg: None,
            should_quit: false,
            attention: HashSet::new(),
            next_id: 0,
            tx,
            ipc,
            win,
            term_size,
        };
        // Restore the previous tabs/splits/panes when enabled ("continue where
        // you left off"). Skipped on first run (nothing saved yet). Falls back
        // to a single fresh shell if there is no usable snapshot.
        let restored = app.config.restore_session
            && app.config.setup_complete
            && app.restore_session(persist::load());
        if !restored {
            let (cmd, title) = app.shell_cmd(None);
            let id = app.spawn_session(cmd, title.clone(), false, Some(PaneKind::Shell))?;
            app.tabs.push(Tab::new(title, id));
        }
        Ok(app)
    }

    // ---- layout ----------------------------------------------------------

    /// Recompute pane rects for every tab and resize PTYs whose size changed.
    /// Must run before each draw so the UI and the PTYs agree.
    pub fn sync_layout(&mut self, w: u16, h: u16) {
        self.term_size = (w, h);
        let main_h = h.saturating_sub(2);
        let mut pane_area = Rect {
            x: 0,
            y: 1,
            width: w,
            height: main_h,
        };
        if self.explorer_visible {
            let ew = self.config.explorer_width.min(w / 2);
            self.explorer_rect = Rect {
                x: 0,
                y: 1,
                width: ew,
                height: main_h,
            };
            pane_area = Rect {
                x: ew,
                y: 1,
                width: w - ew,
                height: main_h,
            };
        }

        let mut active_rects = Vec::new();
        let mut active_bordered = false;
        for (ti, tab) in self.tabs.iter().enumerate() {
            let mut rects = Vec::new();
            if tab.zoomed && tab.root.contains(tab.focus) {
                rects.push((tab.focus, pane_area));
            } else {
                tab.root.rects(pane_area, &mut rects);
            }
            let bordered = rects.len() > 1;
            for (id, r) in &rects {
                let inner = if bordered {
                    r.inner(Margin::new(1, 1))
                } else {
                    *r
                };
                if let Some(s) = self.sessions.get_mut(id) {
                    s.resize(inner.height.max(1), inner.width.max(1));
                } else if let Some(v) = self.viewers.get_mut(id) {
                    // viewers are always framed; lay text out in the content area
                    let c = Viewer::content_rect(*r, true);
                    v.relayout(c.width);
                    v.set_view_h(c.height);
                }
            }
            if ti == self.active_tab {
                active_rects = rects;
                active_bordered = bordered;
            }
        }
        self.pane_rects = active_rects;
        self.bordered = active_bordered;

        // the focused pane is being looked at: drop any attention marker on it
        if self.focus == Focus::Pane
            && let Some(f) = self.tabs.get(self.active_tab).map(|t| t.focus)
        {
            self.attention.remove(&f);
        }

        // visible sessions are about to be rendered: re-arm their output events
        for (id, _) in &self.pane_rects {
            if let Some(s) = self.sessions.get(id) {
                s.dirty.store(false, Ordering::Release);
            }
        }
    }

    // ---- events ----------------------------------------------------------

    pub fn handle_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Input(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                self.handle_key(k);
            }
            AppEvent::Input(Event::Paste(s)) => self.paste(&s),
            AppEvent::Input(Event::Mouse(m)) => self.handle_mouse(&m),
            AppEvent::Input(_) => {}
            AppEvent::PtyOutput => {} // wake-up only; the main loop redraws
            AppEvent::PtyExited(id) => {
                let mut close = self.config.auto_close_exited;
                let Some(s) = self.sessions.get_mut(&id) else {
                    return;
                };
                s.mark_exited();
                close = close || s.auto_close;
                if close {
                    self.remove_session(id);
                }
            }
            AppEvent::Attention { pane, kind, message } => self.on_attention(pane, kind, message),
            AppEvent::FocusPane(pane) => self.focus_pane(pane),
            AppEvent::UpdateChecked(info) => self.update = Some(*info),
            AppEvent::UpdateResult(res) => {
                self.updating = false;
                match res {
                    Ok(msg) => self.status_msg = Some(msg),
                    Err(e) => {
                        self.status_msg = Some(format!("update failed: {e}"));
                        // fall back to the manual download page
                        if let Some(u) = &self.update {
                            let _ = opener::open(&u.release_url);
                        }
                    }
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        self.status_msg = None;
        if self.wizard.is_some() {
            self.handle_wizard_key(&key);
            return;
        }
        if self.update_overlay.is_some() {
            self.handle_update_key(&key);
            return;
        }
        if self.show_help {
            self.show_help = false;
            return;
        }
        if self.settings_open {
            self.handle_settings_key(&key);
            return;
        }
        if self.renaming.is_some() {
            self.handle_rename_key(&key);
            return;
        }
        if self.copy_mode.is_some() {
            self.handle_copy_mode_key(&key);
            return;
        }

        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if self.passthrough {
            if alt && key.code == KeyCode::Char('L') {
                self.passthrough = false;
                self.status_msg = Some("passthrough off".into());
                return;
            }
            self.forward_key(&key);
            return;
        }

        if alt {
            match key.code {
                KeyCode::Char('q') => self.close_pane(),
                KeyCode::Char('Q') => self.close_tab(),
                KeyCode::Char('t') => {
                    let cwd = self.focused_cwd();
                    let (cmd, title) = self.shell_cmd(cwd.as_deref());
                    self.new_tab_with(cmd, title, Some(PaneKind::Shell));
                }
                KeyCode::Char('o') => {
                    self.settings_open = true;
                    self.settings_sel = 0;
                    self.settings_edit = None;
                }
                KeyCode::Char('r') => {
                    if let Some(t) = self.tabs.get(self.active_tab) {
                        self.renaming = Some(t.name.clone());
                    }
                }
                KeyCode::Char('z') => {
                    if let Some(t) = self.tabs.get_mut(self.active_tab) {
                        t.zoomed = !t.zoomed;
                    }
                }
                KeyCode::Char('e') => self.toggle_explorer(),
                KeyCode::Char('s') => {
                    let cwd = self.focused_cwd();
                    let (cmd, title) = self.shell_cmd(cwd.as_deref());
                    self.split_with(SPLIT_DIR, cmd, title, false, Some(PaneKind::Shell));
                }
                KeyCode::Char('c') => {
                    let cwd = self.focused_cwd();
                    let (cmd, title) = self.claude_cmd(cwd.as_deref(), false);
                    self.split_with(
                        SPLIT_DIR,
                        cmd,
                        title,
                        false,
                        Some(PaneKind::Claude { bypass: false }),
                    );
                }
                KeyCode::Char('b') => {
                    let cwd = self.focused_cwd();
                    let (cmd, title) = self.claude_cmd(cwd.as_deref(), true);
                    self.split_with(
                        SPLIT_DIR,
                        cmd,
                        title,
                        false,
                        Some(PaneKind::Claude { bypass: true }),
                    );
                }
                KeyCode::Char('y') => self.enter_copy_mode(),
                KeyCode::Char('u') => self.open_update_overlay(),
                KeyCode::Char('?') => self.show_help = true,
                KeyCode::Char('L') => {
                    self.passthrough = true;
                    self.status_msg = Some("passthrough on — Alt+Shift+L to exit".into());
                }
                KeyCode::Char(c @ '1'..='9') => self.goto_tab(c as usize - '1' as usize),
                KeyCode::Char('[') | KeyCode::Char(',') => self.prev_tab(),
                KeyCode::Char(']') | KeyCode::Char('.') => self.next_tab(),
                KeyCode::Char('h') => self.nav(NavDir::Left),
                KeyCode::Char('j') => self.nav(NavDir::Down),
                KeyCode::Char('k') => self.nav(NavDir::Up),
                KeyCode::Char('l') => self.nav(NavDir::Right),
                KeyCode::Left if shift => self.resize_split(NavDir::Left),
                KeyCode::Right if shift => self.resize_split(NavDir::Right),
                KeyCode::Up if shift => self.resize_split(NavDir::Up),
                KeyCode::Down if shift => self.resize_split(NavDir::Down),
                KeyCode::Left => self.nav(NavDir::Left),
                KeyCode::Right => self.nav(NavDir::Right),
                KeyCode::Up => self.nav(NavDir::Up),
                KeyCode::Down => self.nav(NavDir::Down),
                KeyCode::PageUp => self.scroll_focused(true),
                KeyCode::PageDown => self.scroll_focused(false),
                _ => {
                    if self.focus == Focus::Pane {
                        self.forward_key(&key);
                    }
                }
            }
            return;
        }

        match self.focus {
            Focus::Explorer => self.handle_explorer_key(&key),
            Focus::Pane => match self.focused_viewer_id() {
                Some(id) => self.handle_viewer_key(id, &key),
                None => self.forward_key(&key),
            },
        }
    }

    /// Scroll keys for a focused built-in viewer pane. The pane closes with
    /// `Alt+q` like any other pane (handled in the Alt branch).
    fn handle_viewer_key(&mut self, id: SessionId, key: &KeyEvent) {
        let Some(v) = self.viewers.get_mut(&id) else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => v.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => v.scroll_by(-1),
            KeyCode::Char(' ') | KeyCode::Char('f') | KeyCode::PageDown => v.page(true),
            KeyCode::Char('b') | KeyCode::PageUp => v.page(false),
            KeyCode::Char('g') | KeyCode::Home => v.scroll_top(),
            KeyCode::Char('G') | KeyCode::End => v.scroll_bottom(),
            _ => {}
        }
    }

    // ---- settings ----------------------------------------------------------

    /// Raw value of a settings row, used for display and as the edit seed.
    pub fn setting_value(&self, idx: usize) -> String {
        match idx {
            0 => self.config.show_explorer_on_start.to_string(),
            1 => self.config.auto_close_exited.to_string(),
            2 => self.config.explorer_width.to_string(),
            3 => self.config.scrollback.to_string(),
            4 => self.config.default_shell.clone().unwrap_or_default(),
            5 => self.config.claude_command.clone(),
            6 => self.config.viewer.label().to_string(),
            7 => self.config.pager.clone(),
            8 => self.config.check_updates.to_string(),
            9 => self.config.notifications.to_string(),
            10 => self.config.restore_session.to_string(),
            _ => String::new(),
        }
    }

    fn handle_settings_key(&mut self, key: &KeyEvent) {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if alt && key.code == KeyCode::Char('o') {
            self.settings_open = false;
            self.settings_edit = None;
            return;
        }
        if let Some(buf) = self.settings_edit.as_mut() {
            match key.code {
                KeyCode::Enter => {
                    let val = self.settings_edit.take().unwrap_or_default();
                    self.commit_setting(&val);
                }
                KeyCode::Esc => self.settings_edit = None,
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
                {
                    buf.push(c);
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.settings_open = false,
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings_sel = self.settings_sel.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings_sel = (self.settings_sel + 1).min(SETTINGS.len() - 1);
            }
            KeyCode::Enter | KeyCode::Char(' ') => match SETTINGS[self.settings_sel].kind {
                SettingKind::Bool => {
                    self.toggle_bool_setting();
                    self.save_config();
                }
                SettingKind::Choice => {
                    self.cycle_choice_setting();
                    self.save_config();
                }
                SettingKind::Action => self.run_setting_action(),
                _ => self.settings_edit = Some(self.setting_value(self.settings_sel)),
            },
            _ => {}
        }
    }

    fn toggle_bool_setting(&mut self) {
        match self.settings_sel {
            0 => {
                self.config.show_explorer_on_start = !self.config.show_explorer_on_start;
                // mirror the new startup default right away for visible feedback
                self.explorer_visible = self.config.show_explorer_on_start;
                if !self.explorer_visible && self.focus == Focus::Explorer {
                    self.focus = Focus::Pane;
                }
            }
            1 => self.config.auto_close_exited = !self.config.auto_close_exited,
            8 => self.config.check_updates = !self.config.check_updates,
            9 => {
                self.config.notifications = !self.config.notifications;
                let res = if self.config.notifications {
                    notify::install_hooks()
                } else {
                    notify::uninstall_hooks()
                };
                if let Err(e) = res {
                    self.status_msg = Some(format!("notify hooks: {e}"));
                } else {
                    self.status_msg = Some(
                        if self.config.notifications {
                            "notifications on — restart panes to take effect"
                        } else {
                            "notifications off"
                        }
                        .into(),
                    );
                }
            }
            10 => self.config.restore_session = !self.config.restore_session,
            _ => {}
        }
    }

    fn cycle_choice_setting(&mut self) {
        if self.settings_sel == 6 {
            self.config.viewer = self.config.viewer.toggled();
        }
    }

    /// Run the action for an `Action`-kind settings row (index 11: re-run setup).
    fn run_setting_action(&mut self) {
        if self.settings_sel == 11 {
            self.settings_open = false;
            self.settings_edit = None;
            self.open_setup_wizard();
        }
    }

    fn commit_setting(&mut self, val: &str) {
        let val = val.trim();
        match self.settings_sel {
            2 => match val.parse::<u16>() {
                Ok(n) if (10..=120).contains(&n) => self.config.explorer_width = n,
                _ => {
                    self.status_msg = Some("explorer width must be 10..120".into());
                    return;
                }
            },
            3 => match val.parse::<usize>() {
                Ok(n) if n <= 1_000_000 => self.config.scrollback = n,
                _ => {
                    self.status_msg = Some("scrollback must be a number up to 1000000".into());
                    return;
                }
            },
            4 => {
                self.config.default_shell = if val.is_empty() {
                    None
                } else {
                    Some(val.to_string())
                };
            }
            5 => {
                if val.is_empty() {
                    self.status_msg = Some("claude command cannot be empty".into());
                    return;
                }
                self.config.claude_command = val.to_string();
            }
            7 => {
                if val.is_empty() {
                    self.status_msg = Some("pager cannot be empty".into());
                    return;
                }
                self.config.pager = val.to_string();
            }
            _ => {}
        }
        self.save_config();
    }

    fn save_config(&mut self) {
        match self.config.save() {
            Ok(path) => self.status_msg = Some(format!("saved to {}", path.display())),
            Err(e) => self.status_msg = Some(format!("save failed: {e}")),
        }
    }

    // ---- setup wizard ------------------------------------------------------

    /// Open the first-run setup wizard (also reachable later from settings).
    pub fn open_setup_wizard(&mut self) {
        self.wizard = Some(Wizard::new(&self.config));
    }

    fn handle_wizard_key(&mut self, key: &KeyEvent) {
        let Some(mut w) = self.wizard.take() else {
            return;
        };
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let typed = |code: KeyCode| match code {
            KeyCode::Char(c) if !alt && !ctrl => Some(c),
            _ => None,
        };
        let mut finish = false;

        if key.code == KeyCode::Esc {
            finish = true;
        } else if alt && key.code == KeyCode::Left {
            w.prev();
        } else {
            match w.step {
                Step::Welcome => {
                    if key.code == KeyCode::Enter {
                        w.next();
                    }
                }
                Step::Claude => match key.code {
                    KeyCode::Enter => w.next(),
                    KeyCode::Backspace => {
                        w.claude.pop();
                    }
                    code => {
                        if let Some(c) = typed(code) {
                            w.claude.push(c);
                        }
                    }
                },
                Step::Folders => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => w.picker.move_sel(-1),
                    KeyCode::Down | KeyCode::Char('j') => w.picker.move_sel(1),
                    KeyCode::Right | KeyCode::Char('l') => w.picker.enter(),
                    KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => w.picker.up(),
                    KeyCode::Char(' ') => w.toggle_highlighted_root(),
                    KeyCode::Char('.') => w.picker.toggle_hidden(),
                    KeyCode::Enter => w.next(),
                    _ => {}
                },
                Step::Shell => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        w.shell_sel = w.shell_sel.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        w.shell_sel = (w.shell_sel + 1).min(w.shells.len().saturating_sub(1));
                    }
                    KeyCode::Enter => w.next(),
                    _ => {}
                },
                Step::Display => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        w.display_sel = w.display_sel.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        w.display_sel = (w.display_sel + 1).min(1);
                    }
                    KeyCode::Char(' ') => {
                        if w.display_sel == 0 {
                            w.viewer = w.viewer.toggled();
                        } else {
                            w.explorer_on_start = !w.explorer_on_start;
                        }
                    }
                    KeyCode::Enter => w.next(),
                    _ => {}
                },
                Step::Finish => {
                    if key.code == KeyCode::Enter {
                        finish = true;
                    }
                }
            }
        }

        if finish {
            self.finish_wizard(&w);
        } else {
            self.wizard = Some(w);
        }
    }

    fn finish_wizard(&mut self, w: &Wizard) {
        w.apply(&mut self.config);
        if let Err(e) = self.config.save() {
            self.status_msg = Some(format!("config save failed: {e}"));
        } else {
            self.status_msg = Some("setup complete".into());
        }
        self.explorer = Explorer::new(self.config.explorer_roots.clone());
        self.explorer_visible = self.config.show_explorer_on_start;
        self.wizard = None;
    }

    // ---- updates -----------------------------------------------------------

    fn open_update_overlay(&mut self) {
        if self.update.is_some() {
            self.update_overlay = Some(UpdateOverlay {
                mode: UpdateMode::Prompt,
                scroll: 0,
            });
        } else {
            self.status_msg = Some("mipoco is up to date".into());
        }
    }

    fn handle_update_key(&mut self, key: &KeyEvent) {
        let Some(mode) = self.update_overlay.as_ref().map(|o| o.mode) else {
            return;
        };
        match mode {
            UpdateMode::Prompt => match key.code {
                KeyCode::Char('u') => {
                    self.update_overlay = None;
                    self.start_update();
                }
                KeyCode::Char('c') => {
                    if let Some(o) = self.update_overlay.as_mut() {
                        o.mode = UpdateMode::Changelog;
                        o.scroll = 0;
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.update_overlay = None,
                _ => {}
            },
            UpdateMode::Changelog => {
                let Some(o) = self.update_overlay.as_mut() else {
                    return;
                };
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down => o.scroll = o.scroll.saturating_add(1),
                    KeyCode::Char('k') | KeyCode::Up => o.scroll = o.scroll.saturating_sub(1),
                    KeyCode::PageDown | KeyCode::Char(' ') => o.scroll = o.scroll.saturating_add(10),
                    KeyCode::PageUp | KeyCode::Char('b') => o.scroll = o.scroll.saturating_sub(10),
                    KeyCode::Esc | KeyCode::Char('q') => o.mode = UpdateMode::Prompt,
                    _ => {}
                }
            }
        }
    }

    fn start_update(&mut self) {
        let Some(info) = self.update.clone() else {
            return;
        };
        if self.updating {
            return;
        }
        self.updating = true;
        self.status_msg = Some("downloading update…".into());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = update::apply(&info).map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::UpdateResult(res));
        });
    }

    fn handle_rename_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                if let Some(name) = self.renaming.take()
                    && !name.is_empty()
                    && let Some(t) = self.tabs.get_mut(self.active_tab)
                {
                    t.name = name;
                }
            }
            KeyCode::Esc => self.renaming = None,
            KeyCode::Backspace => {
                if let Some(buf) = self.renaming.as_mut() {
                    buf.pop();
                }
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                if let Some(buf) = self.renaming.as_mut() {
                    buf.push(c);
                }
            }
            _ => {}
        }
    }

    fn handle_explorer_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.explorer.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.explorer.move_sel(1),
            KeyCode::PageUp => self.explorer.move_sel(-10),
            KeyCode::PageDown => self.explorer.move_sel(10),
            KeyCode::Enter => {
                let Some(e) = self.explorer.selected_entry() else {
                    return;
                };
                if e.is_dir {
                    self.explorer.toggle_expand();
                } else {
                    let p = e.path.clone();
                    self.execute_file(&p);
                }
            }
            KeyCode::Right | KeyCode::Char('l') => self.explorer.expand(),
            KeyCode::Left | KeyCode::Char('h') => self.explorer.collapse_or_parent(),
            KeyCode::Char('x') => {
                if let Some(e) = self.explorer.selected_entry()
                    && !e.is_dir
                {
                    let p = e.path.clone();
                    self.execute_file(&p);
                }
            }
            KeyCode::Char('s') => {
                let d = self.explorer.target_dir();
                let (cmd, title) = self.shell_cmd(Some(&d));
                self.new_tab_with(cmd, title, Some(PaneKind::Shell));
            }
            KeyCode::Char('c') => {
                let d = self.explorer.target_dir();
                let (cmd, title) = self.claude_cmd(Some(&d), false);
                self.new_tab_with(cmd, title, Some(PaneKind::Claude { bypass: false }));
            }
            KeyCode::Char('b') => {
                let d = self.explorer.target_dir();
                let (cmd, title) = self.claude_cmd(Some(&d), true);
                self.new_tab_with(cmd, title, Some(PaneKind::Claude { bypass: true }));
            }
            KeyCode::Char('v') => {
                if let Some(e) = self.explorer.selected_entry()
                    && !e.is_dir
                {
                    let p = e.path.clone();
                    self.view_file(&p);
                }
            }
            KeyCode::Char('S') => {
                let d = self.explorer.target_dir();
                let (cmd, title) = self.shell_cmd(Some(&d));
                self.split_with(SPLIT_DIR, cmd, title, false, Some(PaneKind::Shell));
            }
            KeyCode::Char('C') => {
                let d = self.explorer.target_dir();
                let (cmd, title) = self.claude_cmd(Some(&d), false);
                self.split_with(
                    SPLIT_DIR,
                    cmd,
                    title,
                    false,
                    Some(PaneKind::Claude { bypass: false }),
                );
            }
            KeyCode::Char('B') => {
                let d = self.explorer.target_dir();
                let (cmd, title) = self.claude_cmd(Some(&d), true);
                self.split_with(
                    SPLIT_DIR,
                    cmd,
                    title,
                    false,
                    Some(PaneKind::Claude { bypass: true }),
                );
            }
            KeyCode::Char('.') => self.explorer.toggle_hidden(),
            KeyCode::Char('R') => self.explorer.rebuild(),
            KeyCode::Backspace | KeyCode::Char('-') => self.explorer.collapse_all(),
            KeyCode::Esc => self.focus = Focus::Pane,
            _ => {}
        }
    }

    // ---- copy mode & mouse -------------------------------------------------

    fn enter_copy_mode(&mut self) {
        if self.focus != Focus::Pane {
            return;
        }
        let Some(id) = self.focused_session_id() else {
            return;
        };
        let Some(sess) = self.sessions.get(&id) else {
            return;
        };
        let row = sess.parser.lock().screen().cursor_position().0;
        self.copy_mode = Some(CopyMode {
            cursor_row: row,
            anchor_row: None,
        });
    }

    fn handle_copy_mode_key(&mut self, key: &KeyEvent) {
        let Some(mut cm) = self.copy_mode.take() else {
            return;
        };
        let Some(id) = self.focused_session_id() else {
            return;
        };
        let Some(sess) = self.sessions.get(&id) else {
            return;
        };
        let (rows, cols) = sess.parser.lock().screen().size();
        let last = rows.saturating_sub(1);
        let mut yanked: Option<String> = None;
        let mut done = false;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => done = true,
            KeyCode::Up | KeyCode::Char('k') => {
                if cm.cursor_row > 0 {
                    cm.cursor_row -= 1;
                } else if cm.anchor_row.is_none() {
                    // at the top edge with nothing marked: scroll into scrollback
                    let mut p = sess.parser.lock();
                    let cur = p.screen().scrollback();
                    p.screen_mut().set_scrollback(cur + 1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if cm.cursor_row < last {
                    cm.cursor_row += 1;
                } else if cm.anchor_row.is_none() {
                    let mut p = sess.parser.lock();
                    let cur = p.screen().scrollback();
                    p.screen_mut().set_scrollback(cur.saturating_sub(1));
                }
            }
            KeyCode::PageUp => cm.cursor_row = cm.cursor_row.saturating_sub(rows / 2),
            KeyCode::PageDown => cm.cursor_row = (cm.cursor_row + rows / 2).min(last),
            KeyCode::Char('g') | KeyCode::Home => cm.cursor_row = 0,
            KeyCode::Char('G') | KeyCode::End => cm.cursor_row = last,
            KeyCode::Char(' ') | KeyCode::Char('v') => cm.anchor_row = Some(cm.cursor_row),
            KeyCode::Enter | KeyCode::Char('y') => {
                let (a, b) = cm.range();
                yanked = Some(sess.parser.lock().screen().contents_between(a, 0, b, cols));
                done = true;
            }
            _ => {}
        }
        if done {
            // leave the view live again
            if let Some(sess) = self.sessions.get(&id) {
                sess.parser.lock().screen_mut().set_scrollback(0);
            }
        } else {
            self.copy_mode = Some(cm);
        }
        if let Some(text) = yanked {
            self.copy_text(&text);
        }
    }

    fn copy_text(&mut self, text: &str) {
        let text = tidy_copy(text);
        if text.trim().is_empty() {
            self.status_msg = Some("nothing to copy".into());
            return;
        }
        let n = text.lines().count();
        let via = clipboard::copy(&text);
        let plural = if n == 1 { "" } else { "s" };
        self.status_msg = Some(format!("copied {n} line{plural} via {via}"));
    }

    /// Inner (content) rect of a pane in the active tab.
    fn pane_inner(&self, id: SessionId) -> Option<Rect> {
        let (_, r) = self.pane_rects.iter().find(|(pid, _)| *pid == id)?;
        Some(if self.bordered {
            r.inner(Margin::new(1, 1))
        } else {
            *r
        })
    }

    /// Pane whose content area contains the given screen position.
    fn pane_at(&self, pos: Position) -> Option<(SessionId, Rect)> {
        self.pane_rects.iter().find_map(|(id, r)| {
            let inner = if self.bordered {
                r.inner(Margin::new(1, 1))
            } else {
                *r
            };
            inner.contains(pos).then_some((*id, inner))
        })
    }

    fn handle_mouse(&mut self, m: &MouseEvent) {
        if self.show_help {
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.show_help = false;
            }
            return;
        }
        if self.settings_open || self.renaming.is_some() {
            return;
        }

        // an in-progress drag selection captures the mouse until release
        if self.mouse_sel.is_some() {
            match m.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.update_mouse_sel(m.column, m.row);
                    return;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.finish_mouse_sel();
                    return;
                }
                _ => self.mouse_sel = None,
            }
        }

        let pos = Position::new(m.column, m.row);
        if self.explorer_visible && self.explorer_rect.contains(pos) {
            self.explorer_mouse(m);
            return;
        }
        let Some((id, inner)) = self.pane_at(pos) else {
            return;
        };

        if matches!(m.kind, MouseEventKind::Down(_)) {
            self.focus = Focus::Pane;
            if let Some(tab) = self.tabs.get_mut(self.active_tab)
                && tab.root.contains(id)
            {
                tab.focus = id;
            }
        }

        // built-in viewer panes: wheel scrolls the document
        if let Some(v) = self.viewers.get_mut(&id) {
            match m.kind {
                MouseEventKind::ScrollUp => v.scroll_by(-3),
                MouseEventKind::ScrollDown => v.scroll_by(3),
                _ => {}
            }
            return;
        }

        let (prow, pcol) = (m.row - inner.y, m.column - inner.x);
        let Some(sess) = self.sessions.get_mut(&id) else {
            return;
        };
        let (mode, enc, alt_screen, app_cursor) = {
            let p = sess.parser.lock();
            let s = p.screen();
            (
                s.mouse_protocol_mode(),
                s.mouse_protocol_encoding(),
                s.alternate_screen(),
                s.application_cursor(),
            )
        };

        // the inner application asked for mouse events: forward them
        if mode != vt100::MouseProtocolMode::None
            && self.copy_mode.is_none()
            && sess.exited.is_none()
        {
            if let Some(bytes) = encode_mouse(m, pcol, prow, mode, enc) {
                sess.write_bytes(&bytes);
            }
            return;
        }

        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.mouse_sel = Some(MouseSel {
                    id,
                    start: (prow, pcol),
                    end: (prow, pcol),
                    dragged: false,
                });
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = matches!(m.kind, MouseEventKind::ScrollUp);
                if alt_screen {
                    // no scrollback there; send arrows like other multiplexers
                    let code = if up { KeyCode::Up } else { KeyCode::Down };
                    if let Some(bytes) =
                        encode_key(&KeyEvent::new(code, KeyModifiers::NONE), app_cursor)
                    {
                        for _ in 0..3 {
                            sess.write_bytes(&bytes);
                        }
                    }
                } else {
                    let mut p = sess.parser.lock();
                    let cur = p.screen().scrollback();
                    let new = if up { cur + 3 } else { cur.saturating_sub(3) };
                    p.screen_mut().set_scrollback(new);
                }
            }
            _ => {}
        }
    }

    fn update_mouse_sel(&mut self, col: u16, row: u16) {
        let Some(sel) = &self.mouse_sel else {
            return;
        };
        let Some(inner) = self.pane_inner(sel.id) else {
            self.mouse_sel = None;
            return;
        };
        let row = row.clamp(inner.y, inner.y + inner.height.saturating_sub(1)) - inner.y;
        let col = col.clamp(inner.x, inner.x + inner.width.saturating_sub(1)) - inner.x;
        if let Some(sel) = self.mouse_sel.as_mut() {
            sel.end = (row, col);
            sel.dragged = true;
        }
    }

    fn finish_mouse_sel(&mut self) {
        let Some(sel) = self.mouse_sel.take() else {
            return;
        };
        if !sel.dragged {
            return; // plain click: focus change only
        }
        let (s, e) = sel.ordered();
        let Some(sess) = self.sessions.get(&sel.id) else {
            return;
        };
        let text = sess
            .parser
            .lock()
            .screen()
            .contents_between(s.0, s.1, e.0, e.1 + 1);
        self.copy_text(&text);
    }

    fn explorer_mouse(&mut self, m: &MouseEvent) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = Focus::Explorer;
                let inner = self.explorer_rect.inner(Margin::new(1, 1));
                if !inner.contains(Position::new(m.column, m.row)) {
                    return;
                }
                let idx = self.explorer.offset + usize::from(m.row - inner.y);
                if idx >= self.explorer.entries.len() {
                    return;
                }
                if idx == self.explorer.selected {
                    // second click on the selected entry acts like Enter
                    let e = &self.explorer.entries[idx];
                    if e.is_dir {
                        self.explorer.toggle_expand();
                    } else {
                        let p = e.path.clone();
                        self.execute_file(&p);
                    }
                } else {
                    self.explorer.selected = idx;
                }
            }
            MouseEventKind::ScrollUp => self.explorer.move_sel(-3),
            MouseEventKind::ScrollDown => self.explorer.move_sel(3),
            _ => {}
        }
    }

    // ---- pane I/O --------------------------------------------------------

    fn forward_key(&mut self, key: &KeyEvent) {
        let Some(id) = self.focused_session_id() else {
            return;
        };
        if self
            .sessions
            .get(&id)
            .is_some_and(|s| s.exited.is_some())
        {
            self.remove_session(id);
            return;
        }
        let app_cursor = {
            let Some(sess) = self.sessions.get(&id) else {
                return;
            };
            let mut p = sess.parser.lock();
            if p.screen().scrollback() > 0 {
                p.screen_mut().set_scrollback(0);
            }
            p.screen().application_cursor()
        };
        if let Some(bytes) = encode_key(key, app_cursor)
            && let Some(sess) = self.sessions.get_mut(&id)
        {
            sess.write_bytes(&bytes);
        }
    }

    fn paste(&mut self, text: &str) {
        if self.focus != Focus::Pane {
            return;
        }
        let Some(id) = self.focused_session_id() else {
            return;
        };
        let Some(sess) = self.sessions.get_mut(&id) else {
            return;
        };
        if sess.exited.is_some() {
            return;
        }
        let bracketed = sess.parser.lock().screen().bracketed_paste();
        let mut bytes = Vec::with_capacity(text.len() + 12);
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            bytes.extend_from_slice(text.replace('\n', "\r").as_bytes());
        }
        sess.write_bytes(&bytes);
    }

    fn scroll_focused(&mut self, up: bool) {
        let Some(id) = self.focused_session_id() else {
            return;
        };
        if let Some(v) = self.viewers.get_mut(&id) {
            v.page(!up); // PageUp moves toward the top of the document
            return;
        }
        let Some(sess) = self.sessions.get(&id) else {
            return;
        };
        let mut p = sess.parser.lock();
        if p.screen().alternate_screen() {
            return; // no scrollback on the alternate screen
        }
        let half = usize::from(p.screen().size().0 / 2).max(1);
        let cur = p.screen().scrollback();
        let new = if up { cur + half } else { cur.saturating_sub(half) };
        p.screen_mut().set_scrollback(new);
    }

    // ---- sessions / tabs / panes ------------------------------------------

    fn alloc_id(&mut self) -> SessionId {
        self.next_id += 1;
        self.next_id
    }

    fn pane_size_hint(&self) -> (u16, u16) {
        let (w, h) = self.term_size;
        if w < 10 || h < 5 {
            // terminal size unknown or degenerate; sync_layout corrects it next frame
            return (24, 80);
        }
        (h - 2, w)
    }

    fn shell_cmd(&self, cwd: Option<&Path>) -> (CommandBuilder, String) {
        let shell = self.config.shell();
        let title = Path::new(&shell)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| shell.clone());
        let mut cmd = CommandBuilder::new(&shell);
        if let Some(d) = cwd {
            cmd.cwd(d);
        }
        (cmd, title)
    }

    /// Build the command for a claude pane. When `bypass` is set, claude runs
    /// with `--dangerously-skip-permissions` (YOLO mode).
    ///
    /// On Unix the command runs through an interactive login shell
    /// (`$SHELL -ic 'exec …'`) so claude is found on the PATH the user sets in
    /// their shell rc (e.g. `~/.npm-global/bin` in `~/.zshrc`). This matters when
    /// mipoco is launched from a desktop icon, where that PATH addition is absent
    /// and a bare `claude` would fail to spawn.
    fn claude_cmd(&self, cwd: Option<&Path>, bypass: bool) -> (CommandBuilder, String) {
        self.claude_cmd_inner(cwd, bypass, false)
    }

    /// Like `claude_cmd`, but adds `--continue` so claude resumes the most recent
    /// conversation in `cwd`. Used when restoring a session on startup.
    fn claude_cmd_resume(&self, cwd: Option<&Path>, bypass: bool) -> (CommandBuilder, String) {
        self.claude_cmd_inner(cwd, bypass, true)
    }

    fn claude_cmd_inner(
        &self,
        cwd: Option<&Path>,
        bypass: bool,
        resume: bool,
    ) -> (CommandBuilder, String) {
        let mut line = self.config.claude_command.clone();
        if resume {
            line.push_str(" --continue");
        }
        if bypass {
            line.push_str(" --dangerously-skip-permissions");
        }
        let title = if bypass { "claude!".into() } else { "claude".into() };

        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = CommandBuilder::new(self.config.shell());
            c.args(["-ic", &format!("exec {line}")]);
            c
        };
        #[cfg(windows)]
        let mut cmd = {
            // Windows GUI apps inherit the system PATH, so run claude directly.
            let mut parts = line.split_whitespace();
            let prog = parts.next().unwrap_or("claude").to_string();
            let mut c = CommandBuilder::new(&prog);
            for a in parts {
                c.arg(a);
            }
            c
        };
        if let Some(d) = cwd {
            cmd.cwd(d);
        }
        (cmd, title)
    }

    fn spawn_session(
        &mut self,
        mut cmd: CommandBuilder,
        title: String,
        auto_close: bool,
        meta: Option<PaneKind>,
    ) -> Result<SessionId> {
        let id = self.alloc_id();
        // Tag the pane so its Claude hooks can report attention back to us.
        if let Some(ipc) = &self.ipc {
            cmd.env("MIPOCO_SOCK", &ipc.addr);
            cmd.env("MIPOCO_TOKEN", &ipc.token);
            cmd.env("MIPOCO_PANE", id.to_string());
        }
        let size = self.pane_size_hint();
        let sess = PtySession::spawn(
            id,
            cmd,
            size,
            self.config.scrollback,
            title,
            auto_close,
            self.tx.clone(),
        )?;
        self.sessions.insert(id, sess);
        if let Some(m) = meta {
            self.session_meta.insert(id, m);
        }
        Ok(id)
    }

    fn new_tab_with(&mut self, cmd: CommandBuilder, title: String, meta: Option<PaneKind>) {
        match self.spawn_session(cmd, title.clone(), false, meta) {
            Ok(id) => {
                self.tabs.push(Tab::new(title, id));
                self.active_tab = self.tabs.len() - 1;
                self.focus = Focus::Pane;
            }
            Err(e) => self.status_msg = Some(format!("spawn failed: {e}")),
        }
    }

    fn split_with(
        &mut self,
        dir: SplitDir,
        cmd: CommandBuilder,
        title: String,
        auto_close: bool,
        meta: Option<PaneKind>,
    ) {
        if self.tabs.is_empty() {
            self.new_tab_with(cmd, title, meta);
            return;
        }
        match self.spawn_session(cmd, title, auto_close, meta) {
            Ok(id) => {
                let tab = &mut self.tabs[self.active_tab];
                if tab.root.split(tab.focus, dir, id) {
                    tab.focus = id;
                    tab.zoomed = false;
                    self.focus = Focus::Pane;
                } else {
                    self.sessions.remove(&id);
                }
            }
            Err(e) => self.status_msg = Some(format!("spawn failed: {e}")),
        }
    }

    fn close_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let tab = self.tabs.remove(self.active_tab);
        for id in tab.leaves() {
            self.sessions.remove(&id);
            self.viewers.remove(&id);
            self.session_meta.remove(&id);
        }
        if self.tabs.is_empty() {
            self.should_quit = true;
            return;
        }
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    fn close_pane(&mut self) {
        if let Some(id) = self.focused_session_id() {
            self.remove_session(id);
        }
    }

    pub fn remove_session(&mut self, id: SessionId) {
        self.sessions.remove(&id);
        self.viewers.remove(&id);
        self.session_meta.remove(&id);
        let Some(ti) = self.tabs.iter().position(|t| t.root.contains(id)) else {
            return;
        };
        let root = std::mem::replace(&mut self.tabs[ti].root, PaneNode::Leaf(0));
        match root.remove(id) {
            Some(new_root) => {
                let tab = &mut self.tabs[ti];
                tab.root = new_root;
                tab.zoomed = false;
                if tab.focus == id {
                    tab.focus = tab.root.first_leaf();
                }
            }
            None => {
                self.tabs.remove(ti);
                if self.tabs.is_empty() {
                    self.should_quit = true;
                    return;
                }
                if self.active_tab >= self.tabs.len() {
                    self.active_tab = self.tabs.len() - 1;
                }
            }
        }
    }

    fn nav(&mut self, dir: NavDir) {
        if self.focus == Focus::Explorer {
            if dir == NavDir::Right {
                self.focus = Focus::Pane;
            }
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if let Some(id) = directional_focus(&self.pane_rects, tab.focus, dir) {
            tab.focus = id;
        } else if dir == NavDir::Left && self.explorer_visible {
            self.focus = Focus::Explorer;
        }
    }

    fn resize_split(&mut self, dir: NavDir) {
        let (axis, delta) = match dir {
            NavDir::Left => (SplitDir::Horizontal, -0.05),
            NavDir::Right => (SplitDir::Horizontal, 0.05),
            NavDir::Up => (SplitDir::Vertical, -0.05),
            NavDir::Down => (SplitDir::Vertical, 0.05),
        };
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.root.adjust_ratio(tab.focus, axis, delta);
        }
    }

    fn goto_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active_tab = idx;
            self.focus = Focus::Pane;
        }
    }

    fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
        }
    }

    fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
        }
    }

    fn toggle_explorer(&mut self) {
        if !self.explorer_visible {
            self.explorer_visible = true;
            self.focus = Focus::Explorer;
            self.explorer.rebuild();
        } else if self.focus == Focus::Explorer {
            self.explorer_visible = false;
            self.focus = Focus::Pane;
        } else {
            self.focus = Focus::Explorer;
        }
    }

    fn execute_file(&mut self, path: &Path) {
        match exec::execute(path, &self.config) {
            Ok(ExecOutcome::Opened) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.status_msg = Some(format!("opened {name} with system handler"));
            }
            Ok(ExecOutcome::Run { cmd, title }) => {
                self.split_with(SPLIT_DIR, cmd, title, true, None);
            }
            Ok(ExecOutcome::View(p)) => self.view_file(&p),
            Err(e) => self.status_msg = Some(format!("exec failed: {e}")),
        }
    }

    /// Open a text/markdown file in a split pane next to the current one.
    /// In `builtin` viewer mode it opens mipoco's reader (word-wrapped, with
    /// margins); in `external` mode it spawns a pager (glow/bat/less).
    fn view_file(&mut self, path: &Path) {
        match self.config.viewer {
            ViewerMode::Builtin => self.open_builtin_viewer(path),
            ViewerMode::External => {
                let (cmd, title) = exec::view(path, &self.config);
                self.split_with(SPLIT_DIR, cmd, title, true, None);
            }
        }
    }

    fn open_builtin_viewer(&mut self, path: &Path) {
        let Some(id) = self.create_viewer(path) else {
            self.status_msg = Some(format!("cannot view {}", path.display()));
            return;
        };
        self.place_viewer(id);
    }

    /// Read a file into a built-in viewer and register it (with its path, so the
    /// viewer can be restored on the next launch), returning the new id. Does
    /// NOT place it in the layout — see `place_viewer`.
    fn create_viewer(&mut self, path: &Path) -> Option<SessionId> {
        let bytes = std::fs::read(path).ok()?;
        let content = String::from_utf8_lossy(&bytes);
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "view".into());
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        let viewer = Viewer::new(title, &content, WrapMode::for_ext(&ext));
        let id = self.alloc_id();
        self.viewers.insert(id, viewer);
        self.session_meta.insert(
            id,
            PaneKind::Viewer {
                path: path.to_path_buf(),
            },
        );
        Some(id)
    }

    /// Place an existing viewer id as a split next to the focused pane (or in a
    /// new tab when none exists).
    fn place_viewer(&mut self, id: SessionId) {
        let name = self
            .viewers
            .get(&id)
            .map(|v| v.title.clone())
            .unwrap_or_default();
        if self.tabs.is_empty() {
            self.tabs.push(Tab::new(name, id));
            self.active_tab = self.tabs.len() - 1;
            self.focus = Focus::Pane;
            return;
        }
        let tab = &mut self.tabs[self.active_tab];
        if tab.root.split(tab.focus, SPLIT_DIR, id) {
            tab.focus = id;
            tab.zoomed = false;
            self.focus = Focus::Pane;
        } else {
            self.viewers.remove(&id);
            self.session_meta.remove(&id);
        }
    }

    // ---- session restore ("continue where you left off") ------------------

    /// Serialize the current layout to a JSON snapshot for restore-on-restart.
    /// Returns None when there is nothing worth saving (no restorable panes) so
    /// callers never overwrite a good snapshot with an empty one.
    pub fn snapshot_json(&self) -> Option<String> {
        let snap = self.snapshot_session();
        if snap.tabs.is_empty() {
            return None;
        }
        serde_json::to_string_pretty(&snap).ok()
    }

    fn snapshot_session(&self) -> persist::SavedSession {
        let mut tabs = Vec::new();
        for tab in &self.tabs {
            if let Some(root) = self.snapshot_node(&tab.root, tab.focus) {
                tabs.push(persist::SavedTab {
                    name: tab.name.clone(),
                    zoomed: tab.zoomed,
                    root,
                });
            }
        }
        let active_tab = if tabs.is_empty() {
            0
        } else {
            self.active_tab.min(tabs.len() - 1)
        };
        persist::SavedSession {
            tabs,
            active_tab,
            explorer_visible: self.explorer_visible,
        }
    }

    /// Reduce a live layout node to its serializable mirror. Transient panes
    /// (no session_meta) are dropped and their split collapses to the sibling.
    fn snapshot_node(&self, node: &PaneNode, focus: SessionId) -> Option<persist::SavedNode> {
        match node {
            PaneNode::Leaf(id) => {
                let kind = self.session_meta.get(id)?.clone();
                let cwd = self.sessions.get(id).and_then(|s| s.cwd());
                Some(persist::SavedNode::Leaf {
                    pane: kind,
                    cwd,
                    focused: *id == focus,
                })
            }
            PaneNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let f = self.snapshot_node(first, focus);
                let s = self.snapshot_node(second, focus);
                match (f, s) {
                    (Some(f), Some(s)) => Some(persist::SavedNode::Split {
                        dir: *dir,
                        ratio: *ratio,
                        first: Box::new(f),
                        second: Box::new(s),
                    }),
                    (Some(n), None) | (None, Some(n)) => Some(n),
                    (None, None) => None,
                }
            }
        }
    }

    /// Rebuild tabs/panes from a saved snapshot. Returns true when at least one
    /// tab was restored (so the caller can fall back to a fresh shell otherwise).
    fn restore_session(&mut self, saved: persist::SavedSession) -> bool {
        for stab in saved.tabs {
            let mut focus_id = None;
            if let Some(root) = self.build_restore_node(&stab.root, &mut focus_id) {
                let focus = focus_id.unwrap_or_else(|| root.first_leaf());
                self.tabs.push(Tab {
                    name: stab.name,
                    root,
                    focus,
                    zoomed: stab.zoomed,
                });
            }
        }
        if self.tabs.is_empty() {
            return false;
        }
        self.active_tab = saved.active_tab.min(self.tabs.len() - 1);
        self.explorer_visible = saved.explorer_visible;
        true
    }

    fn build_restore_node(
        &mut self,
        node: &persist::SavedNode,
        focus_out: &mut Option<SessionId>,
    ) -> Option<PaneNode> {
        match node {
            persist::SavedNode::Leaf { pane, cwd, focused } => {
                let id = self.spawn_restored_pane(pane, cwd.as_deref())?;
                if *focused {
                    *focus_out = Some(id);
                }
                Some(PaneNode::Leaf(id))
            }
            persist::SavedNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let f = self.build_restore_node(first, focus_out);
                let s = self.build_restore_node(second, focus_out);
                match (f, s) {
                    (Some(f), Some(s)) => Some(PaneNode::Split {
                        dir: *dir,
                        ratio: *ratio,
                        first: Box::new(f),
                        second: Box::new(s),
                    }),
                    (Some(n), None) | (None, Some(n)) => Some(n),
                    (None, None) => None,
                }
            }
        }
    }

    /// Recreate a single restored pane, returning its new id (None if it can't
    /// be respawned, e.g. a viewer whose file is gone).
    fn spawn_restored_pane(&mut self, kind: &PaneKind, cwd: Option<&Path>) -> Option<SessionId> {
        match kind {
            PaneKind::Shell => {
                let (cmd, title) = self.shell_cmd(cwd);
                self.spawn_session(cmd, title, false, Some(PaneKind::Shell))
                    .ok()
            }
            PaneKind::Claude { bypass } => {
                let (cmd, title) = self.claude_cmd_resume(cwd, *bypass);
                self.spawn_session(cmd, title, false, Some(PaneKind::Claude { bypass: *bypass }))
                    .ok()
            }
            PaneKind::Viewer { path } => self.create_viewer(path),
        }
    }

    // ---- attention notifications ------------------------------------------

    /// A pane reported it needs attention. Mark it and pop a desktop
    /// notification — unless you're already looking right at it.
    fn on_attention(&mut self, pane: SessionId, kind: AttentionKind, message: String) {
        if !self.config.notifications || !self.sessions.contains_key(&pane) {
            return;
        }
        let looking = self.focus == Focus::Pane
            && self.tabs.get(self.active_tab).map(|t| t.focus) == Some(pane);
        if looking {
            return;
        }
        self.attention.insert(pane);
        let title = self
            .sessions
            .get(&pane)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        notify::show(kind, &title, &message, pane, self.tx.clone());
    }

    /// Jump to the tab + pane behind a clicked notification and raise the window.
    fn focus_pane(&mut self, pane: SessionId) {
        if let Some(ti) = self.tabs.iter().position(|t| t.root.contains(pane)) {
            self.active_tab = ti;
            self.tabs[ti].focus = pane;
            self.focus = Focus::Pane;
            self.attention.remove(&pane);
            notify::window::raise(&self.win);
        }
    }

    fn focused_session_id(&self) -> Option<SessionId> {
        self.tabs.get(self.active_tab).map(|t| t.focus)
    }

    /// The focused pane's id if it is a built-in viewer (not a PTY session).
    fn focused_viewer_id(&self) -> Option<SessionId> {
        let id = self.focused_session_id()?;
        self.viewers.contains_key(&id).then_some(id)
    }

    fn focused_cwd(&self) -> Option<PathBuf> {
        let id = self.focused_session_id()?;
        self.sessions.get(&id)?.cwd()
    }
}

/// Box-drawing vertical bars that TUIs (e.g. Claude) frame their panels with.
const FRAME_BARS: &[char] = &['│', '┃', '╎', '╏', '┆', '┇', '┊', '┋'];

/// Tidy text grabbed off a terminal screen before it hits the clipboard:
/// drop right-edge padding and any box-drawing frame the inner app drew around
/// the selection, while preserving real indentation and ASCII `|` (markdown,
/// code). Only a single outer frame bar per side is removed.
fn tidy_copy(text: &str) -> String {
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            let mut s = line.trim_end();
            // trailing frame bar + its padding
            if let Some(rest) = s.strip_suffix(FRAME_BARS) {
                s = rest.trim_end();
            }
            // leading frame bar — only strip when one is actually present, so
            // ordinary indented lines keep their leading spaces
            let after_indent = s.trim_start_matches(' ');
            if let Some(rest) = after_indent.strip_prefix(FRAME_BARS) {
                rest.strip_prefix(' ').unwrap_or(rest).to_string()
            } else {
                s.to_string()
            }
        })
        .collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::tidy_copy;

    #[test]
    fn strips_box_frame() {
        assert_eq!(tidy_copy("│ hello │"), "hello");
        assert_eq!(tidy_copy("│ hello world         │"), "hello world");
    }

    #[test]
    fn keeps_code_indentation() {
        assert_eq!(tidy_copy("    let x = 5;"), "    let x = 5;");
    }

    #[test]
    fn leaves_ascii_pipes_alone() {
        assert_eq!(tidy_copy("| a | b |"), "| a | b |");
    }

    #[test]
    fn trims_trailing_padding_and_blank_lines() {
        assert_eq!(tidy_copy("foo   \nbar\n   \n"), "foo\nbar");
    }
}
