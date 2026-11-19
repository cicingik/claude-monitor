//! The office view's card of the selected agent, and where it goes beside it.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Widget},
};

use crate::{
    actors::link_rgb,
    format::{age, clock, short_id, short_model},
    scan::{EdgeKind, Kind, truncate},
};

use super::{App, bold, doing, faint, glyph, status_color};

/// The widest a card gets, border and padding included.
const CARD_W: u16 = 56;

impl App {
    /// The character name an agent goes by on the card (its short id, for
    /// one no scan showed).
    pub(super) fn name_of<'a>(&'a self, id: &'a str) -> &'a str {
        self.names.get(id).map_or_else(|| short_id(id), String::as_str)
    }

    /// Another agent on the card: "lead" for a session, else its name.
    fn who(&self, id: &str) -> String {
        match self.agent(id) {
            Some(a) if a.kind == Kind::Session => "lead".into(),
            _ => self.name_of(id).into(),
        }
    }

    /// s with each word that is an agent's id, or 8 or more characters of
    /// its start (cut short, as in "SendMessage: to a5582ca8…"), given as who
    /// it is.
    fn without_ids(&self, s: &str) -> String {
        let named = |w: &str| {
            let bare = w.trim_end_matches('…');
            let id = self.snap.as_ref()?.by_id.keys().find(|id| bare.len() >= 8 && id.starts_with(bare))?;
            Some(self.who(id))
        };
        s.split(' ').map(|w| named(w).unwrap_or_else(|| w.to_string())).collect::<Vec<_>>().join(" ")
    }

    /// The selected agent's card in at most `rows` lines: who it is (its
    /// role), what it did and whom it dealt with,
    /// newest first: the last 3 things it did, up to 8 dealings. No tokens: those are in the tree.
    pub fn card_lines(&self, rows: usize) -> Vec<Line<'static>> {
        let (Some(snap), Some(a)) = (&self.snap, self.agent(&self.focus)) else { return vec![Line::styled("nothing selected", faint())] };
        let now = snap.scanned_at;
        let role = match a.kind {
            Kind::Session => "session lead".to_string(),
            Kind::Subagent if a.agent_type.is_empty() => "subagent".to_string(),
            Kind::Subagent => format!("subagent · {}", a.agent_type),
        };
        // Its name, then what it is about: a subagent's description, a session's title or project.
        let name = match (a.kind, a.title.is_empty()) {
            (Kind::Session, true) => format!("{} ({})", self.name_of(&a.id), a.project),
            (_, true) => self.name_of(&a.id).to_string(),
            (_, false) => format!("{} · {}", self.name_of(&a.id), a.title),
        };
        let mut out = vec![
            Line::from(vec![glyph(a.status), Span::raw(" "), Span::styled(name, bold())]),
            Line::raw(truncate(&format!("role: {role}"), usize::from(CARD_W) - 4)),
            Line::raw(format!("{} · {}", short_model(&a.model), a.status.name())),
            Line::styled(format!("last activity {} ago", age(now, a.last_activity)), faint()),
        ];
        if let Some(now) = doing(a.status) {
            // What it runs or waits on now, wrapped over at most 3 lines.
            let (w, now_style) = (usize::from(CARD_W) - 4, Style::new().fg(status_color(a.status)).add_modifier(Modifier::BOLD));
            let text: Vec<char> = format!("{now} {}", self.without_ids(&a.last_tool)).chars().collect();
            let lines: Vec<String> = text.chunks(w).map(|c| c.iter().collect()).collect();
            for (i, l) in lines.iter().take(3).enumerate() {
                let l = if i == 2 && lines.len() > 3 { truncate(&format!("{l}…"), w) } else { l.clone() };
                out.push(Line::styled(l, now_style));
            }
        }
        let did: Vec<Line<'static>> = a
            .events
            .iter()
            .rev()
            .map(|e| {
                let what = match e.kind {
                    "text" => format!("reply: {}", e.summary),
                    "prompt" => format!("prompt: {}", e.summary),
                    _ => e.summary.clone(),
                };
                let what = self.without_ids(&what);
                Line::from(vec![Span::styled(clock(e.at), faint()), Span::raw(format!(" {what}"))])
            })
            .collect();
        let with: Vec<Line<'static>> = snap
            .edges
            .iter()
            .rev()
            .filter_map(|e| {
                let (verb, other) = if e.from == a.id {
                    (["spawned", "message to", "handback to"], &e.to)
                } else if e.to == a.id {
                    (["spawned by", "message from", "handback from"], &e.from)
                } else {
                    return None;
                };
                let verb = verb[match e.kind {
                    EdgeKind::Spawn => 0,
                    EdgeKind::Message => 1,
                    EdgeKind::Handback => 2,
                }];
                let [r, g, b] = link_rgb(e.kind);
                let mut l = vec![Span::styled(verb, Style::new().fg(Color::Rgb(r, g, b))), Span::raw(format!(" {}", self.who(other))), Span::styled(format!(" {}", age(now, e.at)), faint())];
                if e.kind == EdgeKind::Message && !e.label.is_empty() {
                    l.push(Span::styled(format!(" \"{}\"", e.label), faint()));
                }
                Some(Line::from(l))
            })
            .collect();
        // Fewer events and edges when the pane is short, at least one of each.
        let (mut nd, mut nw) = (did.len().min(3), with.len().min(8));
        while out.len() + 2 + nd.max(1) + nw.max(1) > rows && nd.max(nw) > 1 {
            if nd >= nw { nd -= 1 } else { nw -= 1 }
        }
        for (title, lines, n) in [("did:", did, nd), ("with:", with, nw)] {
            out.push(Line::styled(title, bold()));
            if n == 0 {
                out.push(Line::styled("(none)", faint()));
            }
            out.extend(lines.into_iter().take(n));
        }
        out
    }
}

/// Where a card of size (w, h) goes beside anchor, the cells of its agent:
/// right of it, else left, else below, else above, else at the pane's right
/// edge; always inside pane.
pub fn place((w, h): (u16, u16), anchor: Rect, pane: Rect) -> Rect {
    let (w, h) = (w.min(pane.width), h.min(pane.height));
    let fit_x = |x: u16| x.clamp(pane.x, pane.right() - w);
    let fit_y = |y: u16| y.clamp(pane.y, pane.bottom() - h);
    let (x, y) = if anchor.right().saturating_add(1 + w) <= pane.right() {
        (anchor.right() + 1, fit_y(anchor.y))
    } else if anchor.x > pane.x + w {
        (anchor.x - 1 - w, fit_y(anchor.y))
    } else if anchor.bottom().saturating_add(h) <= pane.bottom() {
        (fit_x(anchor.x), anchor.bottom())
    } else if anchor.y >= pane.y + h {
        (fit_x(anchor.x), anchor.y - h)
    } else {
        (pane.right() - w, fit_y(anchor.y))
    };
    Rect::new(x, y, w, h)
}

/// The card of lines in a box as wide as its widest line (24 to 56 cells),
/// placed beside anchor inside pane: a buffer of exactly its cells, every
/// one written, so it hides what is under it.
pub fn card(lines: Vec<Line<'static>>, anchor: Rect, pane: Rect) -> Buffer {
    let w = u16::try_from(lines.iter().map(Line::width).max().unwrap_or(0) + 4).unwrap_or(u16::MAX).clamp(24, CARD_W);
    let h = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let at = place((w, h), anchor, pane);
    let mut buf = Buffer::empty(at);
    let block = Block::new().borders(Borders::ALL).border_style(Style::new().fg(Color::Indexed(244))).padding(Padding::horizontal(1));
    Paragraph::new(lines).block(block).render(at, &mut buf);
    buf
}
