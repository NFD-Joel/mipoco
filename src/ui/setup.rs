//! Renders the first-run setup wizard overlay. See `crate::setup`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::app::App;
use crate::config::ViewerMode;
use crate::setup::{Step, Wizard};

const DIM: Color = Color::DarkGray;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let Some(w) = &app.wizard else {
        return;
    };

    let width = 72u16.min(area.width.saturating_sub(2));
    let height = 22u16.min(area.height.saturating_sub(2));
    if width < 8 || height < 6 {
        return;
    }
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" mipoco setup ")
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 4 || inner.width < 6 {
        return;
    }

    let pad = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };

    // header
    let header = format!(
        "Step {}/{} — {}",
        w.step.index() + 1,
        Step::ALL.len(),
        step_title(w.step)
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            header,
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))),
        Rect { height: 1, ..pad },
    );
    // footer
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(footer(w), Style::new().fg(DIM)))),
        Rect {
            y: pad.y + inner.height - 1,
            height: 1,
            ..pad
        },
    );
    // body (between header and footer)
    let body = Rect {
        y: pad.y + 2,
        height: inner.height.saturating_sub(3),
        ..pad
    };
    if body.height == 0 {
        return;
    }
    match w.step {
        Step::Folders => render_folders(f, w, body),
        Step::Shell => render_shell(f, w, body),
        _ => f.render_widget(
            Paragraph::new(step_body(w)).wrap(Wrap { trim: false }),
            body,
        ),
    }
}

fn step_title(step: Step) -> &'static str {
    match step {
        Step::Welcome => "Welcome",
        Step::Claude => "Claude Code",
        Step::Folders => "Explorer access",
        Step::Shell => "Default shell",
        Step::Display => "Display",
        Step::Finish => "All set",
    }
}

fn footer(w: &Wizard) -> String {
    let nav = match w.step {
        Step::Welcome => "Enter: begin",
        Step::Claude => "type to edit · Enter: next",
        Step::Folders => "j/k move · l/h in/out · Space select · . hidden · Enter: next",
        Step::Shell => "j/k move · Enter: next",
        Step::Display => "j/k move · Space toggle · Enter: next",
        Step::Finish => "Enter: finish",
    };
    let back = if w.step == Step::Welcome {
        "Esc: skip (use defaults)"
    } else {
        "Alt+←: back · Esc: skip"
    };
    format!("{nav}   ·   {back}")
}

fn field(label: &str, value: &str, editing: bool) -> Line<'static> {
    let shown = if editing {
        format!("{value}▏")
    } else {
        value.to_string()
    };
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::new().fg(Color::Gray)),
        Span::styled(
            shown,
            Style::new().fg(if editing { Color::Yellow } else { Color::White }),
        ),
    ])
}

fn step_body(w: &Wizard) -> Vec<Line<'static>> {
    match w.step {
        Step::Welcome => vec![
            Line::from("Welcome to mipoco — a terminal multiplexer for Claude Code."),
            Line::from(""),
            Line::from("This one-time setup configures the Claude command, which folders"),
            Line::from("the file explorer may browse, your shell, and a few preferences."),
            Line::from(""),
            Line::from(Span::styled(
                "You can change everything later from settings (Alt+o).",
                Style::new().fg(DIM),
            )),
        ],
        Step::Claude => {
            let hint = match &w.claude_detected {
                Some(p) => Span::styled(
                    format!("found on PATH: {p}"),
                    Style::new().fg(Color::Green),
                ),
                None => Span::styled(
                    "claude not found on your PATH — install it, or set the command",
                    Style::new().fg(Color::Yellow),
                ),
            };
            vec![
                Line::from("Command mipoco runs for Claude Code sessions:"),
                Line::from(""),
                field("claude command", &w.claude, true),
                Line::from(""),
                Line::from(hint),
            ]
        }
        Step::Display => {
            let viewer = match w.viewer {
                ViewerMode::Builtin => "builtin (in-app reader)",
                ViewerMode::External => "external (pager: glow/bat/less)",
            };
            let on_start = if w.explorer_on_start { "yes" } else { "no" };
            vec![
                Line::from("A couple of preferences (toggle with Space):"),
                Line::from(""),
                toggle_row("text viewer", viewer, w.display_sel == 0),
                toggle_row("open explorer on start", on_start, w.display_sel == 1),
            ]
        }
        Step::Finish => vec![
            Line::from("Review and finish:"),
            Line::from(""),
            summary("claude", w.claude.trim()),
            summary("folders", &format!("{} allowed", w.roots.len().max(1))),
            summary("shell", &w.shell_label()),
            summary("viewer", w.viewer.label()),
            summary(
                "explorer on start",
                if w.explorer_on_start { "yes" } else { "no" },
            ),
        ],
        // Folders/Shell render their own scrollable lists.
        Step::Folders | Step::Shell => Vec::new(),
    }
}

fn render_folders(f: &mut Frame, w: &Wizard, area: Rect) {
    let p = &w.picker;
    // top line: current directory + selected count
    let head = format!(
        "in {}   ({} selected)",
        tilde(&p.cwd.display().to_string()),
        w.roots.len()
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(head, Style::new().fg(Color::Cyan)))),
        Rect { height: 1, ..area },
    );

    let list = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    let h = list.height as usize;
    let (start, end) = window(p.rows(), p.sel, h);

    let mut lines = Vec::with_capacity(end - start);
    for row in start..end {
        let (path, label) = if row == 0 {
            (p.cwd.clone(), "· this folder".to_string())
        } else {
            let dir = &p.dirs[row - 1];
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (dir.clone(), format!("{name}/"))
        };
        let selected = w.roots.contains(&path);
        let check = if selected { "[x] " } else { "[ ] " };
        let marker = if row == p.sel { "▸ " } else { "  " };
        let mut style = if selected {
            Style::new().fg(Color::Green)
        } else {
            Style::new().fg(Color::Gray)
        };
        if row == p.sel {
            style = if selected {
                Style::new().fg(Color::Black).bg(Color::Green)
            } else {
                Style::new().fg(Color::Black).bg(Color::Cyan)
            };
        }
        lines.push(Line::from(Span::styled(
            format!("{marker}{check}{label}"),
            style,
        )));
    }
    f.render_widget(Paragraph::new(lines), list);
}

fn render_shell(f: &mut Frame, w: &Wizard, area: Rect) {
    f.render_widget(
        Paragraph::new(Line::from("Shell used for new panes:")),
        Rect { height: 1, ..area },
    );
    let list = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    let h = list.height as usize;
    let (start, end) = window(w.shells.len(), w.shell_sel, h);
    let mut lines = Vec::with_capacity(end - start);
    for i in start..end {
        let label = match &w.shells[i] {
            Some(p) => p.clone(),
            None => "default — follow $SHELL automatically".to_string(),
        };
        let sel = i == w.shell_sel;
        let marker = if sel { "▸ " } else { "  " };
        let style = if sel {
            Style::new().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::new().fg(Color::Gray)
        };
        lines.push(Line::from(Span::styled(format!("{marker}{label}"), style)));
    }
    f.render_widget(Paragraph::new(lines), list);
}

fn toggle_row(label: &str, value: &str, sel: bool) -> Line<'static> {
    let marker = if sel { "▸ " } else { "  " };
    Line::from(vec![
        Span::styled(format!("{marker}{label}: "), row_style(sel)),
        Span::styled(value.to_string(), Style::new().fg(Color::White)),
    ])
}

fn summary(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label}: "), Style::new().fg(DIM)),
        Span::styled(value.to_string(), Style::new().fg(Color::White)),
    ])
}

fn row_style(sel: bool) -> Style {
    if sel {
        Style::new().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::new().fg(Color::Gray)
    }
}

/// Visible `[start, end)` slice of `total` rows that keeps `sel` in view.
fn window(total: usize, sel: usize, h: usize) -> (usize, usize) {
    if h == 0 {
        return (0, 0);
    }
    if total <= h {
        return (0, total);
    }
    let start = if sel < h { 0 } else { (sel + 1 - h).min(total - h) };
    (start, (start + h).min(total))
}

fn tilde(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home = home.display().to_string();
        if let Some(rest) = path.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}
