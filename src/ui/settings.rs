use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::{App, SETTINGS, SettingKind};

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let label_w = 26usize;
    let mut lines = vec![Line::from("")];
    for (i, def) in SETTINGS.iter().enumerate() {
        let selected = i == app.settings_sel;
        let editing = selected && app.settings_edit.is_some();
        let value = if editing {
            format!("{}▏", app.settings_edit.clone().unwrap_or_default())
        } else {
            let v = app.setting_value(i);
            if v.is_empty() && def.kind == SettingKind::Text {
                "(auto)".into()
            } else {
                v
            }
        };
        let marker = if selected { "▸ " } else { "  " };
        let label_style = if selected {
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::Gray)
        };
        let value_style = if editing {
            Style::new().fg(Color::Yellow)
        } else if selected {
            Style::new().fg(Color::White)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{:<label_w$}", def.label), label_style),
            Span::styled(value, value_style),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter toggle/edit · Esc close · auto-saved",
        Style::new().fg(Color::DarkGray),
    )));

    let w = 64.min(area.width.saturating_sub(2));
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" settings — config.toml ")
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        rect,
    );
}
