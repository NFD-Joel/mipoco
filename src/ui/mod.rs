pub mod explorer;
pub mod pane;
pub mod settings;
pub mod setup;

use std::sync::atomic::Ordering;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::app::{App, Focus};

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.height < 2 {
        return;
    }
    render_tab_bar(
        f,
        app,
        Rect {
            x: 0,
            y: 0,
            width: area.width,
            height: 1,
        },
    );
    if app.explorer_visible {
        explorer::render(f, app);
    }
    pane::render_all(f, app);
    render_status(
        f,
        app,
        Rect {
            x: 0,
            y: area.height - 1,
            width: area.width,
            height: 1,
        },
    );
    if app.show_help {
        render_help(f, area);
    }
    if app.settings_open {
        settings::render(f, app, area);
    }
    if app.update_overlay.is_some() {
        render_update_overlay(f, app, area);
    }
    if app.wizard.is_some() {
        setup::render(f, app, area);
    }
}

fn render_update_overlay(f: &mut Frame, app: &App, area: Rect) {
    let (Some(ov), Some(info)) = (&app.update_overlay, &app.update) else {
        return;
    };
    use crate::app::UpdateMode;
    let title = format!(" update — v{} available ", info.version);
    let (lines, w, h): (Vec<Line>, u16, u16) = match ov.mode {
        UpdateMode::Prompt => {
            let note = if info.asset_url.is_some() {
                "Download and replace this binary now?"
            } else {
                "No prebuilt binary for your platform — opens the releases page."
            };
            (
                vec![
                    Line::from(""),
                    Line::from(format!("  A new mipoco release is available: v{}", info.version)),
                    Line::from(""),
                    Line::from(format!("  {note}")),
                    Line::from(""),
                    Line::from(Span::styled(
                        "  [u] upgrade    [c] changelog    [Esc] dismiss",
                        Style::new().fg(Color::DarkGray),
                    )),
                ],
                60,
                9,
            )
        }
        UpdateMode::Changelog => {
            let mut out = vec![Line::from("")];
            for raw in info.notes.lines() {
                out.push(Line::from(format!("  {raw}")));
            }
            if info.notes.trim().is_empty() {
                out.push(Line::from(Span::styled(
                    "  (no release notes)",
                    Style::new().fg(Color::DarkGray),
                )));
            }
            out.push(Line::from(""));
            out.push(Line::from(Span::styled(
                "  j/k scroll · Esc back",
                Style::new().fg(Color::DarkGray),
            )));
            (out, 72, 20)
        }
    };

    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    let scroll = if ov.mode == UpdateMode::Changelog {
        ov.scroll
    } else {
        0
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::new().fg(Color::Cyan)),
            ),
        rect,
    );
}

fn render_tab_bar(f: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();
    let mut used = 0usize;
    for (i, tab) in app.tabs.iter().enumerate() {
        let active = i == app.active_tab;
        let activity = !active
            && tab.leaves().iter().any(|id| {
                app.sessions
                    .get(id)
                    .is_some_and(|s| s.dirty.load(Ordering::Relaxed))
            });
        let marker = if activity { "*" } else { "" };
        let dir = app
            .sessions
            .get(&tab.focus)
            .and_then(|s| s.cwd())
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let label = match dir {
            Some(d) => format!(" {}:{} {}{} ", i + 1, tab.name, d, marker),
            None => format!(" {}:{}{} ", i + 1, tab.name, marker),
        };
        let style = if active {
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if activity {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        used += label.chars().count();
        spans.push(Span::styled(label, style));
    }
    let brand = "mipoco ";
    let width = area.width as usize;
    if width > used + brand.len() {
        spans.push(Span::raw(" ".repeat(width - used - brand.len())));
        spans.push(Span::styled(brand, Style::new().fg(Color::DarkGray)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let dim = Style::new().fg(Color::DarkGray);
    let line: Line = if let Some(cm) = &app.copy_mode {
        let (a, b) = cm.range();
        let sel = if a == b {
            format!("line {}", a + 1)
        } else {
            format!("lines {}–{}", a + 1, b + 1)
        };
        Line::from(vec![
            Span::styled(
                " COPY ",
                Style::new().fg(Color::Black).bg(Color::Cyan),
            ),
            Span::styled(
                format!(" {sel} · j/k move · Space mark · y yank · Esc cancel"),
                dim,
            ),
        ])
    } else if app.mouse_sel.as_ref().is_some_and(|s| s.dragged) {
        Line::from(Span::styled(
            " selecting — release to copy ",
            Style::new().fg(Color::Black).bg(Color::Cyan),
        ))
    } else if let Some(buf) = &app.renaming {
        Line::from(vec![
            Span::styled(" rename: ", dim),
            Span::raw(buf.clone()),
            Span::styled("▏", Style::new().fg(Color::Cyan)),
        ])
    } else if app.passthrough {
        Line::from(Span::styled(
            " PASSTHROUGH — Alt+Shift+L to exit ",
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ))
    } else if let Some(msg) = &app.status_msg {
        Line::from(Span::styled(
            format!(" {msg}"),
            Style::new().fg(Color::Yellow),
        ))
    } else if app.focus == Focus::Explorer {
        Line::from(Span::styled(
            " Enter open · s/c shell/claude tab · S/C split · b/B claude bypass · v view · x run · . hidden · R refresh · Bksp top · Esc back",
            dim,
        ))
    } else {
        let mut left = String::new();
        if let Some(tab) = app.tabs.get(app.active_tab) {
            if let Some(s) = app.sessions.get(&tab.focus) {
                left.push_str(&format!(" {}", s.title));
                if let Some(cwd) = s.cwd() {
                    left.push_str(&format!(" · {}", tilde(&cwd.display().to_string())));
                }
            } else if let Some(v) = app.viewers.get(&tab.focus) {
                left.push_str(&format!(" {} · j/k scroll · Alt+q close", v.title));
            }
            if tab.zoomed {
                left.push_str(" · ZOOM");
            }
        }
        let (hint, hint_style) = match &app.update {
            Some(u) => (format!("v{} available · Alt+u ", u.version), Style::new().fg(Color::Yellow)),
            None => ("Alt+? help ".to_string(), dim),
        };
        let width = area.width as usize;
        let used = left.chars().count();
        let mut spans = vec![Span::styled(left, dim)];
        if width > used + hint.len() {
            spans.push(Span::raw(" ".repeat(width - used - hint.len())));
            spans.push(Span::styled(hint, hint_style));
        }
        Line::from(spans)
    };
    f.render_widget(Paragraph::new(line), area);
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

fn render_help(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(""),
        help_line("Tabs", "Alt+t new · Alt+Shift+Q close · Alt+1..9 jump"),
        help_line("", "Alt+,/. prev/next · Alt+r rename"),
        help_line("Panes", "Alt+s shell · Alt+c claude · Alt+b claude bypass"),
        help_line("", "Alt+q close pane"),
        help_line("Focus", "Alt+arrows or Alt+hjkl · Alt+z zoom"),
        help_line("", "Alt+Shift+arrows resize"),
        help_line("Explorer", "Alt+e toggle · Enter open · s/c session here"),
        help_line("", "S/C split · b/B claude bypass · v view file"),
        help_line("", "x run · . hidden · Bksp top"),
        help_line("Scroll", "Alt+PgUp/PgDn or wheel · any input returns live"),
        help_line("Viewer", "j/k or wheel scroll · g/G top/bottom · Alt+q close"),
        help_line("Copy", "Alt+y copy mode · or drag with the mouse"),
        help_line("", "Shift+drag = native terminal selection"),
        help_line("Misc", "Alt+o settings · Alt+u update · Alt+Shift+L passthrough"),
        help_line("", "Alt+? help"),
        Line::from(""),
        Line::from(Span::styled(
            "  everything else goes to the focused terminal",
            Style::new().fg(Color::DarkGray),
        )),
    ];
    let w = 68.min(area.width.saturating_sub(2));
    let inner_w = w.saturating_sub(2).max(1);
    // estimate wrapped rows so narrow terminals still show every line
    let content_rows: u16 = lines
        .iter()
        .map(|l| (l.width() as u16).max(1).div_ceil(inner_w))
        .sum();
    let h = (content_rows + 2).min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .title(" mipoco — keys ")
                    .border_style(Style::new().fg(Color::Cyan)),
            ),
        rect,
    );
}

fn help_line(label: &'static str, text: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<9}"),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(text),
    ])
}
