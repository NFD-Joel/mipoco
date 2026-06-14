//! First-run setup wizard: a modal overlay that walks a new user through the
//! essentials — a welcome, the Claude command, which folders the explorer may
//! browse (a browse-and-select picker), the default shell (chosen from the
//! shells detected on the machine), and a couple of display preferences — then
//! writes them to the config so it never runs again (`Config::setup_complete`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config::{Config, ViewerMode};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Welcome,
    Claude,
    Folders,
    Shell,
    Display,
    Finish,
}

impl Step {
    pub const ALL: [Step; 6] = [
        Step::Welcome,
        Step::Claude,
        Step::Folders,
        Step::Shell,
        Step::Display,
        Step::Finish,
    ];

    pub fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn at(i: usize) -> Step {
        Step::ALL[i.min(Step::ALL.len() - 1)]
    }
}

/// A simple directory browser used to pick the explorer's allowed folders.
pub struct Picker {
    pub cwd: PathBuf,
    /// Sub-directories of `cwd` (sorted, hidden filtered unless `show_hidden`).
    pub dirs: Vec<PathBuf>,
    /// Highlighted row: 0 = "this folder" (`cwd`), 1.. = `dirs[row - 1]`.
    pub sel: usize,
    pub show_hidden: bool,
}

impl Picker {
    fn new(start: PathBuf) -> Self {
        let mut p = Self {
            cwd: start,
            dirs: Vec::new(),
            sel: 0,
            show_hidden: false,
        };
        p.rebuild();
        p
    }

    fn rebuild(&mut self) {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&self.cwd)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter(|p| self.show_hidden || !is_hidden(p))
            .collect();
        dirs.sort_by_key(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
        self.dirs = dirs;
        self.sel = self.sel.min(self.rows().saturating_sub(1));
    }

    /// Total selectable rows: the "this folder" row plus each sub-directory.
    pub fn rows(&self) -> usize {
        self.dirs.len() + 1
    }

    /// Absolute path the highlighted row refers to.
    pub fn highlighted(&self) -> PathBuf {
        if self.sel == 0 {
            self.cwd.clone()
        } else {
            self.dirs[self.sel - 1].clone()
        }
    }

    pub fn move_sel(&mut self, delta: isize) {
        let max = self.rows() as isize - 1;
        self.sel = (self.sel as isize + delta).clamp(0, max) as usize;
    }

    /// Descend into the highlighted sub-directory (no-op on the "this folder" row).
    pub fn enter(&mut self) {
        if self.sel >= 1 {
            self.cwd = self.dirs[self.sel - 1].clone();
            self.sel = 0;
            self.rebuild();
        }
    }

    /// Go to the parent directory, if any.
    pub fn up(&mut self) {
        if let Some(parent) = self.cwd.parent().map(Path::to_path_buf) {
            self.cwd = parent;
            self.sel = 0;
            self.rebuild();
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.rebuild();
    }
}

pub struct Wizard {
    pub step: Step,
    /// Staged claude command (this field is the live edit buffer on its step).
    pub claude: String,
    /// `command -v claude` result, shown as a hint; None when not found.
    pub claude_detected: Option<String>,
    /// Staged explorer allowlist (toggled in the picker).
    pub roots: Vec<PathBuf>,
    pub picker: Picker,
    /// Detected shells; `None` = "use $SHELL automatically".
    pub shells: Vec<Option<String>>,
    pub shell_sel: usize,
    pub viewer: ViewerMode,
    pub explorer_on_start: bool,
    /// Selected row on the Display step: 0 = viewer, 1 = explorer-on-start.
    pub display_sel: usize,
}

impl Wizard {
    pub fn new(config: &Config) -> Self {
        let detected = detect_claude(&config.shell());
        let claude = detected
            .clone()
            .unwrap_or_else(|| config.claude_command.clone());
        let roots = if config.explorer_roots.is_empty() {
            crate::config::default_explorer_roots()
        } else {
            config.explorer_roots.clone()
        };
        let start = dirs::home_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let shells = detect_shells();
        // pre-select the configured shell, else "auto"
        let shell_sel = config
            .default_shell
            .as_ref()
            .and_then(|s| shells.iter().position(|c| c.as_deref() == Some(s.as_str())))
            .unwrap_or(0);
        Self {
            step: Step::Welcome,
            claude,
            claude_detected: detected,
            roots,
            picker: Picker::new(start),
            shells,
            shell_sel,
            viewer: config.viewer,
            explorer_on_start: config.show_explorer_on_start,
            display_sel: 0,
        }
    }

    pub fn next(&mut self) {
        self.step = Step::at(self.step.index() + 1);
    }

    pub fn prev(&mut self) {
        self.step = Step::at(self.step.index().saturating_sub(1));
    }

    /// Toggle the highlighted folder in/out of the allowlist.
    pub fn toggle_highlighted_root(&mut self) {
        let path = self.picker.highlighted();
        if let Some(i) = self.roots.iter().position(|r| *r == path) {
            self.roots.remove(i);
        } else {
            self.roots.push(path);
        }
    }

    pub fn shell_label(&self) -> String {
        match self.shells.get(self.shell_sel).and_then(|s| s.as_ref()) {
            Some(p) => p.clone(),
            None => format!("default — $SHELL ({})", current_shell()),
        }
    }

    /// Write the staged choices into `config` and mark setup complete.
    pub fn apply(&self, config: &mut Config) {
        let claude = self.claude.trim();
        if !claude.is_empty() {
            config.claude_command = claude.to_string();
        }
        config.explorer_roots = if self.roots.is_empty() {
            crate::config::default_explorer_roots()
        } else {
            self.roots.clone()
        };
        config.default_shell = self
            .shells
            .get(self.shell_sel)
            .cloned()
            .flatten()
            .filter(|s| !s.is_empty());
        config.viewer = self.viewer;
        config.show_explorer_on_start = self.explorer_on_start;
        config.setup_complete = true;
    }
}

fn is_hidden(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

fn current_shell() -> String {
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
    #[cfg(windows)]
    {
        "powershell".into()
    }
}

/// First executable named `name` on PATH.
fn which(name: &str) -> Option<String> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .map(|p| p.display().to_string())
}

/// Detect shells installed on this machine. The first entry is `None` ("auto").
#[cfg(not(windows))]
fn detect_shells() -> Vec<Option<String>> {
    let mut paths: Vec<String> = Vec::new();
    if let Ok(text) = std::fs::read_to_string("/etc/shells") {
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with('/') && Path::new(l).exists() {
                paths.push(l.to_string());
            }
        }
    }
    for name in [
        "bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "nu", "elvish", "xonsh",
    ] {
        if let Some(p) = which(name) {
            paths.push(p);
        }
    }
    if let Ok(s) = std::env::var("SHELL")
        && !s.is_empty()
    {
        paths.push(s);
    }
    dedup_into_choices(paths)
}

#[cfg(windows)]
fn detect_shells() -> Vec<Option<String>> {
    let mut paths = Vec::new();
    for name in ["powershell.exe", "pwsh.exe", "cmd.exe"] {
        if let Some(p) = which(name) {
            paths.push(p);
        }
    }
    dedup_into_choices(paths)
}

fn dedup_into_choices(paths: Vec<String>) -> Vec<Option<String>> {
    let mut seen = BTreeSet::new();
    let mut out: Vec<Option<String>> = vec![None]; // "auto"
    for p in paths {
        if seen.insert(p.clone()) {
            out.push(Some(p));
        }
    }
    out
}

/// Best-effort lookup of the `claude` binary on the user's interactive-shell
/// PATH (so profile-only dirs like `~/.npm-global/bin` are searched).
#[cfg(not(windows))]
fn detect_claude(shell: &str) -> Option<String> {
    let out = std::process::Command::new(shell)
        .args(["-ic", "command -v claude"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // an interactive shell may print prompt noise; take the last path-looking line
    let path = text
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())?
        .to_string();
    (path.contains('/')).then_some(path)
}

#[cfg(windows)]
fn detect_claude(_shell: &str) -> Option<String> {
    let out = std::process::Command::new("where")
        .arg("claude")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let path = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .to_string();
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mktree() -> PathBuf {
        let base = std::env::temp_dir().join(format!("mipoco_wiz_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("work")).unwrap();
        std::fs::create_dir_all(base.join("other")).unwrap();
        base
    }

    #[test]
    fn apply_writes_staged_values_and_completes() {
        let cfg0 = Config::default();
        let mut cfg = Config::default();
        let mut w = Wizard::new(&cfg0);
        w.claude = "claude-x".into();
        w.roots = vec![PathBuf::from("/tmp/work")];
        w.shells = vec![None, Some("/bin/fish".into())];
        w.shell_sel = 1;
        w.viewer = ViewerMode::External;
        w.explorer_on_start = true;
        w.apply(&mut cfg);
        assert!(cfg.setup_complete);
        assert_eq!(cfg.claude_command, "claude-x");
        assert_eq!(cfg.explorer_roots, vec![PathBuf::from("/tmp/work")]);
        assert_eq!(cfg.default_shell, Some("/bin/fish".into()));
        assert_eq!(cfg.viewer, ViewerMode::External);
        assert!(cfg.show_explorer_on_start);
    }

    #[test]
    fn auto_shell_maps_to_none() {
        let cfg0 = Config::default();
        let mut cfg = Config::default();
        let mut w = Wizard::new(&cfg0);
        w.shell_sel = 0; // the "auto" entry
        w.apply(&mut cfg);
        assert_eq!(cfg.default_shell, None);
    }

    #[test]
    fn empty_roots_fall_back_to_default() {
        let cfg = Config::default();
        let mut out = Config::default();
        let mut w = Wizard::new(&cfg);
        w.roots.clear();
        w.apply(&mut out);
        assert!(!out.explorer_roots.is_empty());
    }

    #[test]
    fn step_navigation_clamps() {
        let cfg = Config::default();
        let mut w = Wizard::new(&cfg);
        w.prev(); // already first
        assert_eq!(w.step, Step::Welcome);
        for _ in 0..10 {
            w.next();
        }
        assert_eq!(w.step, Step::Finish);
    }

    #[test]
    fn picker_lists_navigates_and_toggles() {
        let base = mktree();
        let mut p = Picker::new(base.clone());
        // row 0 is "this folder"; work/ and other/ are listed
        assert_eq!(p.rows(), 3);
        assert_eq!(p.highlighted(), base);
        p.move_sel(1);
        let first = p.highlighted();
        assert!(first.starts_with(&base) && first != base);
        p.enter(); // descend into it
        assert_eq!(p.cwd, first);
        p.up();
        assert_eq!(p.cwd, base);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn toggle_root_adds_and_removes() {
        let base = mktree();
        let cfg = Config::default();
        let mut w = Wizard::new(&cfg);
        w.roots.clear();
        w.picker = Picker::new(base.clone());
        w.toggle_highlighted_root(); // selects base (row 0)
        assert_eq!(w.roots, vec![base.clone()]);
        w.toggle_highlighted_root(); // deselects
        assert!(w.roots.is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn tilde_expands() {
        use crate::config::expand_tilde;
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expand_tilde("~/foo"), home.join("foo"));
            assert_eq!(expand_tilde("~"), home);
        }
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
    }
}
