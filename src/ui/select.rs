//! Selecting text in the tree view with the mouse, inside one pane, to copy it.

use ratatui::{
    Frame,
    crossterm::event::{MouseButton, MouseEventKind},
    layout::{Position, Rect},
    style::Modifier,
};

use super::App;

/// A drag from `from` to `to` (screen cells, in reading order or not) within
/// the inside of one pane; done once the button is up, copied once its text
/// went to main.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Selection {
    pane: Rect,
    from: Position,
    to: Position,
    done: bool,
    copied: bool,
}

impl Selection {
    /// The cells selected, row by row, as terminals select: from the first
    /// cell to the end of its row, whole rows, then up to the last cell.
    fn rows(&self) -> impl Iterator<Item = (u16, u16, u16)> {
        let (a, b) = if (self.from.y, self.from.x) <= (self.to.y, self.to.x) { (self.from, self.to) } else { (self.to, self.from) };
        let p = self.pane;
        (a.y..=b.y).map(move |y| (y, if y == a.y { a.x } else { p.x }, if y == b.y { b.x } else { p.right() - 1 }))
    }
}

impl App {
    /// A mouse event in the tree view: a left press starts a selection in
    /// the pane under it, a drag extends it within that pane, the release
    /// copies it. A click that moves nowhere selects nothing.
    pub fn mouse(&mut self, kind: MouseEventKind, col: u16, row: u16) {
        let at = Position::new(col, row);
        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = self.panes.get().into_iter().find(|p| p.contains(at)).map(|pane| Selection { pane, from: at, to: at, done: false, copied: false });
            }
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
                let Some(s) = &mut self.selection else { return };
                if s.done {
                    return;
                }
                let p = s.pane;
                s.to = Position::new(col.clamp(p.x, p.right() - 1), row.clamp(p.y, p.bottom() - 1));
                if matches!(kind, MouseEventKind::Up(_)) {
                    s.done = true;
                    if s.from == s.to {
                        self.selection = None;
                    }
                }
            }
            _ => {}
        }
    }

    /// Shows the selection on what was just drawn and, once it is done,
    /// takes its text for main to copy, once.
    pub(super) fn draw_selection(&mut self, f: &mut Frame) {
        let Some(s) = self.selection else { return };
        let buf = f.buffer_mut();
        let mut lines = Vec::new();
        for (y, x0, x1) in s.rows() {
            let mut line = String::new();
            for x in x0..=x1 {
                let c = &mut buf[(x, y)];
                line.push_str(c.symbol());
                c.modifier.toggle(Modifier::REVERSED);
            }
            lines.push(line.trim_end().to_string());
        }
        if s.done && !s.copied {
            let text = lines.join("\n");
            self.note = format!("copied {} chars", text.chars().count());
            self.copied = Some(text);
            self.selection = Some(Selection { copied: true, ..s });
        }
    }
}
