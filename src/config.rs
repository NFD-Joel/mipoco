use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How text/markdown files open from the explorer (`v`) and on execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewerMode {
    /// mipoco's built-in reader: word-wrapped, margins, comfortable spacing.
    Builtin,
    /// An external pager in a pane (auto-picks glow/bat when present, else `pager`).
    External,
}

impl ViewerMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewerMode::Builtin => "builtin",
            ViewerMode::External => "external",
        }
    }

    /// Cycle to the other mode (settings toggle).
    pub fn toggled(self) -> Self {
        match self {
            ViewerMode::Builtin => ViewerMode::External,
            ViewerMode::External => ViewerMode::Builtin,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Shell for new panes. Defaults to $SHELL (Unix) or powershell/%COMSPEC% (Windows).
    pub default_shell: Option<String>,
    /// Whether the first-run setup wizard has been completed. When false, the
    /// wizard runs on startup; finishing it sets this true.
    pub setup_complete: bool,
    /// Folders the explorer is allowed to browse (its top level and upper
    /// boundary — it cannot navigate above these). Defaults to the home dir.
    pub explorer_roots: Vec<PathBuf>,
    /// Check GitHub for a newer release on startup and offer to upgrade.
    pub check_updates: bool,
    /// Pop a desktop notification when a Claude pane asks for permission or
    /// finishes. Installs hooks into `~/.claude/settings.json` when enabled.
    pub notifications: bool,
    /// Show the file explorer panel when the app starts.
    pub show_explorer_on_start: bool,
    /// Command used by the explorer's "claude session here" action.
    pub claude_command: String,
    /// How text/markdown files open: the built-in reader or an external pager.
    pub viewer: ViewerMode,
    /// Pager used in `external` viewer mode when neither glow nor bat is found.
    /// Set to e.g. "glow -p" or "bat" to force a specific external viewer.
    pub pager: String,
    /// Scrollback lines kept per pane (bounded; primary screen only).
    pub scrollback: usize,
    pub explorer_width: u16,
    /// Close a pane immediately when its child exits instead of showing [exited].
    pub auto_close_exited: bool,
    /// extension -> runner command for "execute file in pane".
    pub runners: HashMap<String, String>,
    /// Extensions always handed to the OS default opener.
    pub open_with_system: Vec<String>,
    /// Extensions opened in the pager (scrollable viewer) inside a pane.
    pub view_with_pager: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_shell: None,
            setup_complete: false,
            explorer_roots: default_explorer_roots(),
            check_updates: true,
            notifications: true,
            show_explorer_on_start: false,
            claude_command: "claude".into(),
            viewer: ViewerMode::Builtin,
            pager: "less -R".into(),
            scrollback: 5000,
            explorer_width: 32,
            auto_close_exited: false,
            runners: default_runners(),
            open_with_system: [
                "html", "htm", "pdf", "png", "jpg", "jpeg", "gif", "svg", "webp", "mp4", "mp3",
                "webm", "odt", "docx", "xlsx",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            view_with_pager: [
                "md", "markdown", "txt", "text", "log", "rst", "csv", "tsv", "json", "toml",
                "yaml", "yml", "ini", "conf", "cfg", "env", "lock", "diff", "patch", "rs", "go",
                "c", "h", "cpp", "hpp", "java", "css", "scss", "xml", "sql",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

/// Expand a leading `~` to the home directory.
pub(crate) fn expand_tilde(input: &str) -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        if input == "~" {
            return home;
        }
        if let Some(rest) = input.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(input)
}

/// Default explorer allowlist: the user's home directory (or the cwd if home
/// can't be resolved).
pub(crate) fn default_explorer_roots() -> Vec<PathBuf> {
    let dir = dirs::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    vec![dir]
}

fn default_runners() -> HashMap<String, String> {
    let pairs: &[(&str, &str)] = if cfg!(windows) {
        &[
            ("py", "python"),
            ("js", "node"),
            ("mjs", "node"),
            ("ts", "npx tsx"),
            ("ps1", "powershell -ExecutionPolicy Bypass -File"),
            ("bat", "cmd /C"),
            ("cmd", "cmd /C"),
        ]
    } else {
        &[
            ("py", "python3"),
            ("js", "node"),
            ("mjs", "node"),
            ("ts", "npx tsx"),
            ("sh", "bash"),
            ("rb", "ruby"),
            ("pl", "perl"),
            ("php", "php"),
            ("lua", "lua"),
        ]
    };
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mipoco").join("config.toml"))
}

/// Load config; never fails. Returns a warning message when the file is invalid.
pub fn load() -> (Config, Option<String>) {
    let Some(path) = config_path() else {
        return (Config::default(), None);
    };
    if !path.exists() {
        return (Config::default(), None);
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(mut cfg) => {
                // user runners extend the defaults rather than replacing them
                for (k, v) in default_runners() {
                    cfg.runners.entry(k).or_insert(v);
                }
                // allow `~` in hand-written explorer roots
                cfg.explorer_roots = cfg
                    .explorer_roots
                    .iter()
                    .map(|p| expand_tilde(&p.to_string_lossy()))
                    .collect();
                (cfg, None)
            }
            Err(e) => (
                Config::default(),
                Some(format!("config error ({}): {}", path.display(), e)),
            ),
        },
        Err(e) => (
            Config::default(),
            Some(format!("cannot read {}: {}", path.display(), e)),
        ),
    }
}

impl Config {
    /// Persist the current settings. Note: rewrites the file, dropping comments.
    pub fn save(&self) -> anyhow::Result<PathBuf> {
        let path = config_path().ok_or_else(|| anyhow::anyhow!("no config directory found"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(path)
    }

    pub fn shell(&self) -> String {
        if let Some(s) = &self.default_shell {
            return s.clone();
        }
        #[cfg(windows)]
        {
            if which_exists("powershell.exe") {
                return "powershell.exe".into();
            }
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
        }
        #[cfg(not(windows))]
        {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
        }
    }
}

#[cfg(windows)]
fn which_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join(name).exists()))
        .unwrap_or(false)
}
