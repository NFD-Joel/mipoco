use std::path::{Path, PathBuf};

use anyhow::Result;
use portable_pty::CommandBuilder;

use crate::config::Config;

pub enum ExecOutcome {
    /// Handed to the OS default opener (browser, viewer, ...).
    Opened,
    /// Should run inside a mipoco pane.
    Run { cmd: CommandBuilder, title: String },
    /// A text/markdown file to open in the viewer (built-in or external,
    /// decided by `config.viewer` in the app layer).
    View(PathBuf),
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

    if config.view_with_pager.contains(&ext) {
        return Ok(ExecOutcome::View(path.to_path_buf()));
    }

    opener::open(path)?;
    Ok(ExecOutcome::Opened)
}

/// Build the external-pager command for a file (the `external` viewer mode).
/// Auto-picks a purpose-built tool when installed — `glow` for markdown, `bat`
/// for code/text (syntax highlighting + line gutter) — otherwise falls back to
/// the configured `pager` (`less -R`).
pub fn view(path: &Path, config: &Config) -> (CommandBuilder, String) {
    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "view".into());
    let cwd = path.parent().unwrap_or(Path::new("."));
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let pager = external_pager(&ext, &config.pager);
    (pager_cmd(&config.shell(), &pager, path, cwd), title)
}

/// Choose the external pager command for `ext`, preferring installed tools.
fn external_pager(ext: &str, fallback: &str) -> String {
    if matches!(ext, "md" | "markdown") && which("glow") {
        return "glow -p".into();
    }
    // bat ships as `batcat` on Debian/Ubuntu.
    for bat in ["bat", "batcat"] {
        if which(bat) {
            return format!("{bat} --style=full --paging=always");
        }
    }
    fallback.to_string()
}

/// Whether `name` is an executable found on PATH.
fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join(name).is_file()))
        .unwrap_or(false)
}

/// Build the pager command. On Unix it runs through an interactive login shell
/// (`$SHELL -ic`) so a pager installed under a profile-only PATH dir
/// (e.g. ~/.npm-global/bin, ~/go/bin) is found even when mipoco is started from
/// a desktop icon. The file path rides in as `$0`, so it never needs quoting.
#[cfg(not(windows))]
fn pager_cmd(shell: &str, pager: &str, path: &Path, cwd: &Path) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(shell);
    cmd.args(["-ic", &format!("exec {pager} \"$0\"")]);
    cmd.arg(path);
    cmd.cwd(cwd);
    cmd
}

#[cfg(windows)]
fn pager_cmd(_shell: &str, pager: &str, path: &Path, cwd: &Path) -> CommandBuilder {
    let mut cmd = CommandBuilder::new("cmd.exe");
    let line = format!("{} \"{}\"", pager, path.display());
    cmd.args(["/C", &line]);
    cmd.cwd(cwd);
    cmd
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
