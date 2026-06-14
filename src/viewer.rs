//! Built-in text/markdown reader rendered inside a pane.
//!
//! Unlike the external-pager path (which spawns `less`/`bat`/`glow` in a PTY),
//! this reads the file into memory and lays it out with ratatui: words wrap on
//! whitespace (no mid-word cuts), with side margins and top padding so text is
//! comfortable to read instead of pressed against the frame.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Side margin (columns) and top padding (rows) inside the viewer frame.
pub const MARGIN_X: u16 = 2;
pub const PAD_TOP: u16 = 1;
/// Cap the reading column so prose stays comfortable on very wide panes.
const MAX_TEXT_WIDTH: u16 = 96;

/// How a file's lines are laid out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    /// Reflow prose at word boundaries with light markdown styling.
    Markdown,
    /// Reflow prose at word boundaries, no styling.
    Prose,
    /// Preserve whitespace/indentation; wrap only when a line overflows.
    Code,
}

impl WrapMode {
    /// Pick a layout mode from a (lowercased) file extension.
    pub fn for_ext(ext: &str) -> Self {
        match ext {
            "md" | "markdown" | "rst" => WrapMode::Markdown,
            "txt" | "text" | "log" | "" => WrapMode::Prose,
            _ => WrapMode::Code,
        }
    }
}

pub struct Viewer {
    pub title: String,
    source: Vec<String>,
    mode: WrapMode,
    /// Cached wrapped + styled lines for the current `wrap_width`.
    wrapped: Vec<Line<'static>>,
    wrap_width: u16,
    /// Top visible wrapped-line index.
    scroll: usize,
    /// Height of the content area at the last render (for paging / clamping).
    view_h: u16,
}

impl Viewer {
    pub fn new(title: String, content: &str, mode: WrapMode) -> Self {
        // strip a trailing newline so we don't show a phantom blank last line
        let content = content.strip_suffix('\n').unwrap_or(content);
        let source = content.split('\n').map(str::to_string).collect();
        Self {
            title,
            source,
            mode,
            wrapped: Vec::new(),
            wrap_width: 0,
            scroll: 0,
            view_h: 1,
        }
    }

    /// Content rect of a viewer pane: the pane minus its frame, margins and
    /// top padding. Mirrors the `Block` built in `ui::pane` exactly.
    pub fn content_rect(pane: Rect, bordered: bool) -> Rect {
        let border = u16::from(bordered);
        let x = pane.x + border + MARGIN_X;
        let y = pane.y + border + PAD_TOP;
        let width = pane
            .width
            .saturating_sub(2 * border + 2 * MARGIN_X)
            .min(MAX_TEXT_WIDTH);
        let height = pane.height.saturating_sub(2 * border + PAD_TOP);
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// Re-wrap to `width` if it changed since the last layout.
    pub fn relayout(&mut self, width: u16) {
        if width == self.wrap_width && !self.wrapped.is_empty() {
            return;
        }
        self.wrap_width = width;
        self.wrapped = self.build(width as usize);
    }

    /// Record the viewport height and clamp the scroll offset to it.
    pub fn set_view_h(&mut self, h: u16) {
        self.view_h = h.max(1);
        self.clamp();
    }

    fn max_scroll(&self) -> usize {
        self.wrapped.len().saturating_sub(self.view_h as usize)
    }

    fn clamp(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll());
    }

    pub fn page(&mut self, down: bool) {
        let step = (self.view_h.saturating_sub(1)).max(1) as isize;
        self.scroll_by(if down { step } else { -step });
    }

    pub fn scroll_top(&mut self) {
        self.scroll = 0;
    }

    pub fn scroll_bottom(&mut self) {
        self.scroll = self.max_scroll();
    }

    /// The wrapped lines currently visible, given the viewport height.
    pub fn visible(&self) -> &[Line<'static>] {
        let end = (self.scroll + self.view_h as usize).min(self.wrapped.len());
        &self.wrapped[self.scroll.min(end)..end]
    }

    /// Scroll position as a percentage (0..=100), or None when nothing scrolls.
    pub fn percent(&self) -> Option<u16> {
        let max = self.max_scroll();
        if max == 0 {
            return None;
        }
        Some(((self.scroll * 100) / max) as u16)
    }

    fn build(&self, width: usize) -> Vec<Line<'static>> {
        if width == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut in_fence = false;
        for raw in &self.source {
            let is_fence = self.mode == WrapMode::Markdown && raw.trim_start().starts_with("```");
            if is_fence {
                in_fence = !in_fence;
                push_wrapped(&mut out, raw, width, Style::new().fg(Color::DarkGray), true);
                continue;
            }
            let (style, reflow) = match self.mode {
                WrapMode::Code => (Style::new().fg(Color::Gray), false),
                _ if in_fence => (Style::new().fg(Color::Green), false),
                WrapMode::Markdown if is_heading(raw) => (
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    true,
                ),
                _ => (Style::new(), true),
            };
            push_wrapped(&mut out, raw, width, style, !reflow);
        }
        out
    }
}

fn is_heading(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') && t.trim_start_matches('#').starts_with(' ')
}

/// Wrap `raw` to `width` columns and push each piece as a styled `Line`.
/// `preserve` keeps whitespace and wraps by display width; otherwise words
/// reflow on whitespace (collapsing runs).
fn push_wrapped(out: &mut Vec<Line<'static>>, raw: &str, width: usize, style: Style, preserve: bool) {
    let pieces = if preserve {
        wrap_preserve(raw, width)
    } else {
        wrap_reflow(raw, width)
    };
    for p in pieces {
        out.push(Line::from(Span::styled(p, style)));
    }
}

/// Reflow text on whitespace, breaking any word longer than `width`.
fn wrap_reflow(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in text.split_whitespace() {
        let ww = UnicodeWidthStr::width(word);
        if cur.is_empty() {
            place_word(&mut out, &mut cur, &mut cur_w, word, ww, width);
        } else if cur_w + 1 + ww <= width {
            cur.push(' ');
            cur.push_str(word);
            cur_w += 1 + ww;
        } else {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
            place_word(&mut out, &mut cur, &mut cur_w, word, ww, width);
        }
    }
    out.push(cur);
    out
}

/// Start a fresh line with `word`, hard-breaking it if it exceeds `width`.
fn place_word(
    out: &mut Vec<String>,
    cur: &mut String,
    cur_w: &mut usize,
    word: &str,
    ww: usize,
    width: usize,
) {
    if ww <= width {
        *cur = word.to_string();
        *cur_w = ww;
        return;
    }
    let parts = wrap_preserve(word, width);
    let last = parts.len().saturating_sub(1);
    for (i, p) in parts.into_iter().enumerate() {
        if i == last {
            *cur_w = UnicodeWidthStr::width(p.as_str());
            *cur = p;
        } else {
            out.push(p);
        }
    }
}

/// Hard-wrap by display width, preserving every character (incl. whitespace).
fn wrap_preserve(line: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in line.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += cw;
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[&str]) -> String {
        lines.join("\n")
    }

    #[test]
    fn reflow_wraps_on_words_without_cutting() {
        let w = wrap_reflow("the quick brown fox jumps", 9);
        // every piece fits and no word is split
        assert!(w.iter().all(|l| UnicodeWidthStr::width(l.as_str()) <= 9));
        assert_eq!(w.join(" "), "the quick brown fox jumps");
    }

    #[test]
    fn reflow_hard_breaks_overlong_word() {
        let w = wrap_reflow("supercalifragilistic", 5);
        assert!(w.iter().all(|l| l.chars().count() <= 5));
        assert_eq!(w.concat(), "supercalifragilistic");
    }

    #[test]
    fn preserve_keeps_indentation() {
        let w = wrap_preserve("    indented code", 100);
        assert_eq!(w, vec!["    indented code".to_string()]);
    }

    #[test]
    fn scroll_clamps_to_content() {
        let mut v = Viewer::new("t".into(), &text(&["a", "b", "c", "d", "e"]), WrapMode::Prose);
        v.relayout(20);
        v.set_view_h(2);
        v.scroll_bottom();
        assert_eq!(v.scroll, 3); // 5 lines, 2 tall -> top is line index 3
        v.scroll_by(10);
        assert_eq!(v.scroll, 3); // can't go past the end
        v.scroll_by(-100);
        assert_eq!(v.scroll, 0);
    }

    #[test]
    fn markdown_headings_styled() {
        let v = Viewer::new("t".into(), "# Title\nbody", WrapMode::Markdown);
        let lines = v.build(40);
        assert_eq!(lines.len(), 2);
        let heading_style = lines[0].spans[0].style;
        assert!(heading_style.add_modifier.contains(Modifier::BOLD));
    }
}
