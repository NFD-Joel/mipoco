use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Shell for new panes. Defaults to $SHELL (Unix) or powershell/%COMSPEC% (Windows).
    pub default_shell: Option<String>,
    /// Show the file explorer panel when the app starts.
    pub show_explorer_on_start: bool,
    /// Command used by the explorer's "claude session here" action.
    pub claude_command: String,
    /// Scrollback lines kept per pane (bounded; primary screen only).
    pub scrollback: usize,
    pub explorer_width: u16,
    /// Close a pane immediately when its child exits instead of showing [exited].
    pub auto_close_exited: bool,
    /// extension -> runner command for "execute file in pane".
    pub runners: HashMap<String, String>,
    /// Extensions always handed to the OS default opener.
    pub open_with_system: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_shell: None,
            show_explorer_on_start: false,
            claude_command: "claude".into(),
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
        }
    }
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
