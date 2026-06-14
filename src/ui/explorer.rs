use std::path::Path;

use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Focus};

pub fn render(f: &mut Frame, app: &mut App) {
    let area = app.explorer_rect;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = app.focus == Focus::Explorer;
    let max_title = area.width.saturating_sub(4) as usize;
    let title = match app.explorer.roots.as_slice() {
        [one] => format!(" {} ", shorten_path(one, max_title)),
        many => format!(" {} folders ", many.len()),
    };
    let block = Block::bordered().title(title).border_style(if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    });
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let h = inner.height as usize;
    let ex = &mut app.explorer;
    if ex.selected < ex.offset {
        ex.offset = ex.selected;
    }
    if ex.selected >= ex.offset + h {
        ex.offset = ex.selected + 1 - h;
    }

    let width = inner.width as usize;
    let mut lines = Vec::with_capacity(h);
    for (i, e) in ex.entries.iter().enumerate().skip(ex.offset).take(h) {
        let indent = "  ".repeat(e.depth);
        let icon = if e.is_dir {
            if e.expanded { "▾ " } else { "▸ " }
        } else {
            "  "
        };
        let name = e
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut text = format!("{indent}{icon}{name}");
        let len = text.chars().count();
        if len < width {
            text.push_str(&" ".repeat(width - len));
        }
        let mut style = if e.is_dir {
            Style::new().fg(Color::Blue)
        } else {
            Style::new().fg(Color::Gray)
        };
        if i == ex.selected {
            style = if focused {
                Style::new().fg(Color::Black).bg(Color::Cyan)
            } else {
                style.add_modifier(Modifier::REVERSED)
            };
        }
        lines.push(Line::from(Span::styled(text, style)));
    }
    if ex.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (empty)",
            Style::new().fg(Color::DarkGray),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn shorten_path(p: &Path, max: usize) -> String {
    let mut s = p.display().to_string();
    if let Some(home) = dirs::home_dir() {
        let home = home.display().to_string();
        if let Some(rest) = s.strip_prefix(&home) {
            s = format!("~{rest}");
        }
    }
    let len = s.chars().count();
    if len > max && max > 1 {
        let tail: String = s
            .chars()
            .skip(len - (max - 1))
            .collect();
        format!("…{tail}")
    } else {
        s
    }
}
