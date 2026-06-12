use std::path::Path;

use anyhow::Result;
use portable_pty::CommandBuilder;

use crate::config::Config;

pub enum ExecOutcome {
    /// Handed to the OS default opener (browser, viewer, ...).
    Opened,
    /// Should run inside a mipoco pane.
    Run { cmd: CommandBuilder, title: String },
}

pub fn execute(path: &Path, config: &Config) -> Result<ExecOutcome> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    if config.open_with_system.contains(&ext) {
        opener::open(path)?;
        return Ok(ExecOutcome::Opened);
    }

    if let Some(runner) = config.runners.get(&ext) {
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "run".into());
        let cwd = path.parent().unwrap_or(Path::new("."));
        let cmd = runner_cmd(runner, path, cwd);
        return Ok(ExecOutcome::Run { cmd, title });
    }

    opener::open(path)?;
    Ok(ExecOutcome::Opened)
}

/// Wrap the runner so the pane shows the exit code and waits for Enter
/// instead of closing the instant the script finishes.
#[cfg(not(windows))]
fn runner_cmd(runner: &str, path: &Path, cwd: &Path) -> CommandBuilder {
    let mut cmd = CommandBuilder::new("/bin/sh");
    // the file path is passed as $0 so it never needs shell quoting
    let script = format!(
        "{runner} \"$0\"; ec=$?; printf '\\n[exit: %s] press Enter to close' \"$ec\"; read -r _"
    );
    cmd.args(["-c", &script]);
    cmd.arg(path);
    cmd.cwd(cwd);
    cmd
}

#[cfg(windows)]
fn runner_cmd(runner: &str, path: &Path, cwd: &Path) -> CommandBuilder {
    let mut cmd = CommandBuilder::new("cmd.exe");
    let line = format!("{} \"{}\" & echo. & pause", runner, path.display());
    cmd.args(["/C", &line]);
    cmd.cwd(cwd);
    cmd
}
