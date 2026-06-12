use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Paragraph};
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::app::{App, Focus};

pub fn render_all(f: &mut Frame, app: &App) {
    let Some(tab) = app.tabs.get(app.active_tab) else {
        return;
    };
    for (id, rect) in &app.pane_rects {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let Some(sess) = app.sessions.get(id) else {
            continue;
        };
        let focused = *id == tab.focus && app.focus == Focus::Pane;

        let inner = if app.bordered {
            let border_style = if focused {
                Style::new().fg(Color::Cyan)
            } else if *id == tab.focus {
                Style::new().fg(Color::Blue)
            } else {
                Style::new().fg(Color::DarkGray)
            };
            let block = Block::bordered()
                .border_style(border_style)
                .title(format!(" {} ", sess.title));
            let inner = block.inner(*rect);
            f.render_widget(block, *rect);
            inner
        } else {
            *rect
        };
        if inner.width == 0 || inner.height == 0 {
            continue;
        }

        let parser = sess.parser.lock();
        let screen = parser.screen();
        let widget =
            PseudoTerminal::new(screen).cursor(Cursor::default().visibility(false));
        f.render_widget(widget, inner);

        // copy-mode rows / mouse selection, as an inclusive cell range
        let sel = if focused && let Some(cm) = &app.copy_mode {
            let (a, b) = cm.range();
            Some(((a, 0), (b, inner.width.saturating_sub(1))))
        } else {
            app.mouse_sel
                .as_ref()
                .filter(|s| s.id == *id && s.dragged)
                .map(|s| s.ordered())
        };
        if let Some((s, e)) = sel {
            let buf = f.buffer_mut();
            let max_row = inner.height.saturating_sub(1);
            let max_col = inner.width.saturating_sub(1);
            for row in s.0..=e.0.min(max_row) {
                let c0 = if row == s.0 { s.1 } else { 0 };
                let c1 = if row == e.0 { e.1.min(max_col) } else { max_col };
                for col in c0..=c1 {
                    if let Some(cell) =
                        buf.cell_mut(Position::new(inner.x + col, inner.y + row))
                    {
                        cell.set_style(Style::new().add_modifier(Modifier::REVERSED));
                    }
                }
            }
        }

        let scrollback = screen.scrollback();
        if scrollback > 0 {
            let tag = format!("[+{scrollback}]");
            let w = tag.len() as u16;
            if inner.width > w {
                f.render_widget(
                    Paragraph::new(tag)
                        .style(Style::new().fg(Color::Black).bg(Color::Yellow)),
                    Rect {
                        x: inner.x + inner.width - w,
                        y: inner.y,
                        width: w,
                        height: 1,
                    },
                );
            }
        }

        if let Some(code) = sess.exited {
            let msg = format!(" [exited: {code}] press any key to close ");
            let w = (msg.chars().count() as u16).min(inner.width);
            f.render_widget(
                Paragraph::new(msg).style(Style::new().fg(Color::Black).bg(Color::Red)),
                Rect {
                    x: inner.x,
                    y: inner.y + inner.height - 1,
                    width: w,
                    height: 1,
                },
            );
        }

        if focused
            && app.copy_mode.is_none()
            && sess.exited.is_none()
            && scrollback == 0
            && !screen.hide_cursor()
        {
            let (row, col) = screen.cursor_position();
            if row < inner.height && col < inner.width {
                f.set_cursor_position(Position::new(inner.x + col, inner.y + row));
            }
        }
    }
}
