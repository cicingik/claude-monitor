//! The keys of the office view and the card of its selected agent, and the
//! tree view with its Activity, Interactions and Usage tabs; the Usage tab
//! starts with every account's 5-hour and 7-day limits.
//! The office view is the picture alone; its only text is
//! the card, while it is open.

use std::collections::{BTreeMap, HashMap, HashSet};

use ratatui::{
    Frame,
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Padding, Paragraph, Widget},
};

use crate::{
    actors::{self, Scene, Walker, link_rgb},
    cast::Cast,
    format::{age, agent_type, clock, fit, hour_minute, human_tokens, label, local, short_duration, short_id, short_model, usage_detail},
    limits::{AccountUsage, Limit, Limits},
    scan::{Agent, EdgeKind, Kind, Snapshot, Status, Usage, days_from_civil, truncate},
};

const TABS: [&str; 3] = ["Activity", "Interactions", "Usage"];
/// The widest a card gets, border and padding included.
const CARD_W: u16 = 56;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Office,
    Tree,
}

/// What the main loop must do after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Rescan,
    Switched, // the view changed: clear the screen and the images
}

pub struct App {
    pub snap: Option<Snapshot>, // None until the first scan
    pub since: i64,
    pub view: View,
    // Tree view.
    collapsed: HashSet<String>,
    selected: String,
    cursor: usize,
    offset: usize,
    tab: usize,
    height: u16, // terminal rows at the last draw
    // Office view.
    pub focus: String, // selected agent id
    pub card: bool,    // the selected agent's card is open
    pub scene: Scene,
    walker: Walker,
    // The accounts' limits on the Usage tab; None shows no Accounts section.
    pub limits: Option<Limits>,
    // What the card calls each agent instead of its id: the file name of
    // the character it is drawn with, without the extension.
    names: HashMap<String, String>,
}

fn header_style() -> Style {
    Style::new().fg(Color::Indexed(212)).add_modifier(Modifier::BOLD)
}
fn faint() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}
fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}
fn status_color(s: Status) -> Color {
    Color::Indexed(match s {
        Status::Running => 42,
        Status::Waiting => 208,
        Status::Active => 220,
        Status::Idle => 244,
        Status::Done => 39,
    })
}
/// How the tree and the card lead the tool call an agent is in.
fn doing(s: Status) -> Option<&'static str> {
    match s {
        Status::Running => Some("now:"),
        Status::Waiting => Some("waits on:"),
        _ => None,
    }
}
fn glyph(s: Status) -> Span<'static> {
    Span::styled(s.glyph(), Style::new().fg(status_color(s)))
}

/// The bubbles help line: "key desc • key desc".
fn help(keys: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, (k, d)) in keys.iter().enumerate() {
        if i > 0 {
            out.push(Span::styled(" • ", faint()));
        }
        out.push(Span::styled(k.to_string(), Style::new().fg(Color::Indexed(250))));
        out.push(Span::styled(format!(" {d}"), faint()));
    }
    out
}

/// The element d steps from cur in ids, wrapping around.
fn cycle(ids: &[String], cur: &str, d: isize) -> String {
    if ids.is_empty() {
        return cur.into();
    }
    match ids.iter().position(|id| id == cur) {
        Some(i) => ids[(i as isize + d).rem_euclid(ids.len() as isize) as usize].clone(),
        None => ids[0].clone(),
    }
}

impl App {
    /// An app showing sessions since `since` ago; seed picks the visitors' rounds.
    pub fn new(since: i64, seed: u64) -> App {
        App {
            snap: None,
            since,
            view: View::Office,
            collapsed: HashSet::new(),
            selected: String::new(),
            cursor: 0,
            offset: 0,
            tab: 0,
            height: 30,
            focus: String::new(),
            card: false,
            scene: Scene::default(),
            walker: Walker::new(seed),
            limits: None,
            names: HashMap::new(),
        }
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        self.snap = Some(snap);
        self.sync_cursor();
    }

    fn agent(&self, id: &str) -> Option<&Agent> {
        self.snap.as_ref()?.by_id.get(id)
    }

    /// Lays out the office as of now (Unix nanoseconds) and walks it to sec
    /// (a steady clock). A selected agent no longer drawn hands the focus to
    /// the first one, and its card closes.
    pub fn refresh_scene(&mut self, now: i64, sec: f64, cast: &mut Cast) {
        let Some(snap) = &self.snap else { return };
        let mut sc = actors::layout(snap, now, &self.scene);
        self.walker.step(&mut sc, sec);
        if !sc.roster.contains(&self.focus) {
            self.card = false;
            self.focus = sc.roster.first().cloned().unwrap_or_default();
        }
        // One pick over every agent of the scan, drawn or not, and every
        // actor (the pet too): the sprite it is drawn with names it on the card.
        let kinds: HashMap<String, String> = snap
            .by_id
            .values()
            .map(|a| (a.id.clone(), if a.kind == Kind::Session { "session".into() } else { a.agent_type.clone() }))
            .chain(sc.actors.iter().map(|a| (a.id.clone(), a.kind.clone())))
            .collect();
        let picks: HashMap<&str, usize> = kinds.keys().map(String::as_str).zip(cast.pick(kinds.iter().map(|(id, k)| (id.as_str(), k.as_str())))).collect();
        self.names = picks.iter().map(|(&id, &p)| (id.to_string(), cast.name(p).to_string())).collect();
        for a in &mut sc.actors {
            (a.sprite, a.selected) = (picks[a.id.as_str()], a.id == self.focus);
        }
        self.scene = sc;
    }

    /// Handles a key press.
    pub fn key(&mut self, k: KeyEvent) -> Action {
        let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
        if k.code == KeyCode::Esc && self.card {
            self.card = false;
            return Action::None;
        }
        if ctrl_c || matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
            return Action::Quit;
        }
        if k.code == KeyCode::Char('r') {
            if let Some(l) = &self.limits {
                l.refresh();
            }
            return Action::Rescan;
        }
        match self.view {
            View::Office => self.office_key(k.code),
            View::Tree => self.tree_key(k.code),
        }
    }

    /// Handles a click on the office: on an agent (its id) selects it and
    /// opens its card; anywhere else closes the card.
    pub fn click(&mut self, on: Option<String>) {
        match on {
            Some(id) => (self.focus, self.card) = (id, true),
            None => self.card = false,
        }
    }

    fn office_key(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Char('i') => self.card = !self.card,
            KeyCode::Char('v') => {
                (self.view, self.card) = (View::Tree, false);
                return Action::Switched;
            }
            KeyCode::Enter => {
                (self.view, self.card) = (View::Tree, false);
                if let Some(a) = self.agent(&self.focus).cloned() {
                    let mut p = a.parent_id.clone();
                    while !p.is_empty() {
                        self.collapsed.remove(&p);
                        p = self.agent(&p).map(|x| x.parent_id.clone()).unwrap_or_default();
                    }
                    self.selected = a.id;
                    self.sync_cursor();
                }
                return Action::Switched;
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => self.focus = cycle(&self.scene.roster, &self.focus, -1),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.focus = cycle(&self.scene.roster, &self.focus, 1),
            _ => {}
        }
        Action::None
    }

    fn tree_key(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(a) = self.agent(&self.selected).filter(|a| !a.children.is_empty()) {
                    let id = a.id.clone();
                    if !self.collapsed.remove(&id) {
                        self.collapsed.insert(id);
                    }
                    self.sync_cursor();
                }
            }
            KeyCode::Tab => self.tab = (self.tab + 1) % TABS.len(),
            KeyCode::Char(c @ '1'..='3') => self.tab = c as usize - '1' as usize,
            KeyCode::Char('v') => {
                self.view = View::Office;
                self.walker.reset();
                if self.focus.is_empty() {
                    self.focus = self.selected.clone();
                }
                return Action::Switched;
            }
            _ => {}
        }
        Action::None
    }

    /// The tree flattened, skipping children of collapsed agents.
    fn rows(&self) -> Vec<(&Agent, usize)> {
        let mut out = Vec::new();
        let Some(snap) = &self.snap else { return out };
        fn add<'a>(snap: &'a Snapshot, collapsed: &HashSet<String>, id: &str, depth: usize, out: &mut Vec<(&'a Agent, usize)>) {
            let Some(a) = snap.by_id.get(id) else { return };
            out.push((a, depth));
            if !collapsed.contains(id) {
                for c in &a.children {
                    add(snap, collapsed, c, depth + 1, out);
                }
            }
        }
        for r in &snap.roots {
            add(snap, &self.collapsed, r, 0, &mut out);
        }
        out
    }

    fn tree_height(&self) -> usize {
        usize::from(self.height.saturating_sub(2).max(4)) - 2
    }

    fn move_cursor(&mut self, d: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let cursor = (self.cursor as isize + d).clamp(0, rows.len() as isize - 1) as usize;
        self.selected = rows[cursor].0.id.clone();
        self.cursor = cursor;
        self.scroll_to_cursor();
    }

    /// Re-finds the selected agent after the rows changed. A hidden or
    /// vanished selection moves to its nearest visible ancestor, else the first row.
    fn sync_cursor(&mut self) {
        let rows = self.rows();
        let idx: HashMap<&str, usize> = rows.iter().enumerate().map(|(i, (a, _))| (a.id.as_str(), i)).collect();
        let mut id = self.selected.clone();
        let mut found = None;
        while !id.is_empty() {
            if let Some(&i) = idx.get(id.as_str()) {
                found = Some((i, id));
                break;
            }
            match self.agent(&id) {
                Some(a) => id = a.parent_id.clone(),
                None => break,
            }
        }
        let first = rows.first().map(|(a, _)| a.id.clone()).unwrap_or_default();
        (self.cursor, self.selected) = found.unwrap_or((0, first));
        self.scroll_to_cursor();
    }

    fn scroll_to_cursor(&mut self) {
        let h = self.tree_height();
        if self.cursor < self.offset {
            self.offset = self.cursor;
        }
        if self.cursor >= self.offset + h {
            self.offset = self.cursor + 1 - h;
        }
    }

    /// Draws the tree view.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        self.height = area.height;
        // The pane may have changed since the cursor last moved, while the office was shown.
        self.scroll_to_cursor();
        self.draw_tree(f, area);
    }

    fn line(f: &mut Frame, area: Rect, y: u16, spans: Vec<Span<'static>>) {
        if y < area.bottom() {
            f.render_widget(Paragraph::new(Line::from(spans)), Rect::new(area.x, y, area.width, 1));
        }
    }

    fn draw_tree(&self, f: &mut Frame, area: Rect) {
        let header = match &self.snap {
            None => vec![Span::styled("claude-monitor", header_style()), Span::raw("  scanning…")],
            Some(snap) => {
                let running = snap.by_id.values().filter(|a| a.status == Status::Running).count();
                let waiting = snap.by_id.values().filter(|a| a.status == Status::Waiting).count();
                let since = if self.since > 0 { short_duration(self.since) } else { "all".into() };
                vec![
                    Span::styled("claude-monitor", header_style()),
                    Span::raw(format!(
                        "  sessions {} · agents {} · running {running} · waiting {waiting}  │  scanned {}  │  since {since}",
                        snap.roots.len(),
                        snap.by_id.len(),
                        clock(snap.scanned_at)
                    )),
                ]
            }
        };
        Self::line(f, area, area.y, header);
        let body_h = area.height.saturating_sub(2).max(4).min(area.height.saturating_sub(1));
        let lw = (area.width / 2).max(30).min(area.width.saturating_sub(24).max(30)).min(area.width);
        let rw = area.width.saturating_sub(lw);
        let block = || Block::new().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::new().fg(Color::Indexed(240)));
        let left = Rect::new(area.x, area.y + 1, lw, body_h);
        let right = Rect::new(area.x + lw, area.y + 1, rw, body_h);
        f.render_widget(Paragraph::new(self.tree_lines(usize::from(lw.saturating_sub(2)))).block(block()), left);
        if rw > 2 {
            f.render_widget(Paragraph::new(self.detail_lines(usize::from(rw - 2))).block(block()), right);
        }
        let keys = [("↑/k", "up"), ("↓/j", "down"), ("enter", "collapse/expand"), ("tab", "next tab"), ("1-3", "tab"), ("v", "office"), ("r", "rescan"), ("q", "quit")];
        Self::line(f, area, area.y + 1 + body_h, help(&keys));
    }

    fn tree_lines(&self, w: usize) -> Vec<Line<'static>> {
        let rows = self.rows();
        if rows.is_empty() {
            return vec![Line::styled("no sessions in the time window", faint())];
        }
        let now = self.snap.as_ref().map_or(0, |s| s.scanned_at);
        let end = (self.offset + self.tree_height()).min(rows.len());
        (self.offset..end)
            .map(|i| {
                let (a, depth) = rows[i];
                let caret = match (a.children.is_empty(), self.collapsed.contains(&a.id)) {
                    (true, _) => " ",
                    (false, true) => "▸",
                    (false, false) => "▾",
                };
                let prefix = format!("{}{caret} ", "  ".repeat(depth));
                let mut cols = format!(" {} {:>6} {:>4}", fit(&short_model(&a.model), 10), human_tokens(a.usage.total()), age(now, a.last_activity));
                if w >= 60 {
                    cols = format!(" {}{cols}", fit(agent_type(a), 15));
                }
                let name = if a.kind == Kind::Session { format!("{} [{}]", label(a), a.config) } else { label(a) };
                let title = fit(&name, w.saturating_sub(prefix.chars().count() + 2 + cols.chars().count()));
                if i == self.cursor {
                    let sel = Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD);
                    return Line::styled(format!("{prefix}{} {title}{cols}", a.status.glyph()), sel);
                }
                Line::from(vec![Span::styled(prefix, faint()), glyph(a.status), Span::raw(format!(" {title}")), Span::styled(cols, faint())])
            })
            .collect()
    }

    fn detail_lines(&self, w: usize) -> Vec<Line<'static>> {
        let mut tabs = Vec::new();
        for (i, n) in TABS.iter().enumerate() {
            if i > 0 {
                tabs.push(Span::raw(" "));
            }
            let style = if i == self.tab { Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD) } else { faint() };
            tabs.push(Span::styled(format!(" {} {n} ", i + 1), style));
        }
        let mut out = vec![Line::from(tabs), Line::styled("─".repeat(w), faint())];
        if self.tab == 2 {
            out.extend(self.accounts(w));
        }
        let (Some(snap), Some(a)) = (&self.snap, self.agent(&self.selected)) else {
            out.push(Line::styled("nothing selected", faint()));
            return out;
        };
        out.extend(match self.tab {
            1 => self.interactions(snap, a),
            2 => self.usage(snap, a, w),
            _ => self.activity(snap, a),
        });
        out
    }

    fn activity(&self, snap: &Snapshot, a: &Agent) -> Vec<Line<'static>> {
        let mut out = vec![
            Line::from(vec![glyph(a.status), Span::raw(" "), Span::styled(label(a), bold())]),
            Line::raw(format!("{} · {} · {} · {} · {}", agent_type(a), short_model(&a.model), a.status.name(), a.project, a.config)),
            Line::styled(format!("id {}", a.id), faint()),
            Line::raw(format!("turns {} · tokens {} ({})", a.turns, human_tokens(a.usage.total()), usage_detail(a.usage))),
            Line::raw(format!("last activity {} ago", age(snap.scanned_at, a.last_activity))),
        ];
        if let Some(now) = doing(a.status) {
            out.push(Line::styled(format!("{now} {}", a.last_tool), Style::new().fg(status_color(a.status)).add_modifier(Modifier::BOLD)));
        }
        out.push(Line::raw(""));
        out.push(Line::styled("Recent events", bold()));
        if a.events.is_empty() {
            out.push(Line::styled("  (none)", faint()));
        }
        for e in a.events.iter().rev() {
            let kind = match e.kind {
                "tool_use" => "tool",
                "text" => "reply",
                k => k,
            };
            out.push(Line::from(vec![Span::styled(clock(e.at), faint()), Span::raw(format!(" {} {}", fit(kind, 8), e.summary))]));
        }
        out
    }

    /// The short display form of an agent id in edges and the graph.
    fn name(&self, id: &str) -> String {
        match self.agent(id) {
            Some(a) => format!("{} {}", short_id(id), agent_type(a)),
            None => id.into(),
        }
    }

    /// The character name an agent goes by on the card (its short id, for
    /// one no scan showed).
    fn name_of<'a>(&'a self, id: &'a str) -> &'a str {
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

    fn interactions(&self, snap: &Snapshot, a: &Agent) -> Vec<Line<'static>> {
        let mut out = vec![Line::styled(format!("Edges of {}", short_id(&a.id)), bold())];
        let before = out.len();
        for e in &snap.edges {
            let s = if e.from == a.id {
                format!("→ {:<8} to   {}", e.kind.name(), self.name(&e.to))
            } else if e.to == a.id {
                format!("← {:<8} from {}", e.kind.name(), self.name(&e.from))
            } else {
                continue;
            };
            let mut l = vec![Span::raw(format!("  {s}"))];
            if !e.label.is_empty() && e.kind != EdgeKind::Handback {
                l.push(Span::styled(format!(" \"{}\"", e.label), faint()));
            }
            l.push(Span::styled(format!(" {}", hour_minute(e.at)), faint()));
            out.push(Line::from(l));
        }
        if out.len() == before {
            out.push(Line::styled("  (none)", faint()));
        }
        let Some(root) = snap.by_id.get(&a.session_id) else { return out };
        out.push(Line::raw(""));
        out.push(Line::styled("Session graph", bold()));
        out.extend(self.graph(snap, root));
        out
    }

    /// The session tree with spawn labels on the branches and each agent's
    /// outgoing messages and handbacks as annotations.
    fn graph(&self, snap: &Snapshot, root: &Agent) -> Vec<Line<'static>> {
        let mut spawn_label: HashMap<&str, &str> = HashMap::new();
        let mut notes: HashMap<&str, BTreeMap<String, usize>> = HashMap::new();
        for e in &snap.edges {
            match e.kind {
                EdgeKind::Spawn => _ = spawn_label.insert(&e.to, &e.label),
                EdgeKind::Message => *notes.entry(&e.from).or_default().entry(format!("⇢msg {}", short_id(&e.to))).or_default() += 1,
                EdgeKind::Handback => *notes.entry(&e.from).or_default().entry(format!("⇠handback {}", short_id(&e.to))).or_default() += 1,
            }
        }
        let annotate = |id: &str| -> Option<Span<'static>> {
            let n = notes.get(id)?;
            let parts: Vec<String> = n.iter().map(|(k, c)| format!("[{k} ×{c}]")).collect();
            Some(Span::styled(format!(" {}", parts.join(" ")), faint()))
        };
        let mut first = vec![glyph(root.status), Span::raw(format!(" {} session", short_id(&root.id)))];
        first.extend(annotate(&root.id));
        let mut out = vec![Line::from(first)];
        fn rec(app: &App, snap: &Snapshot, a: &Agent, prefix: &str, labels: &HashMap<&str, &str>, annotate: &dyn Fn(&str) -> Option<Span<'static>>, out: &mut Vec<Line<'static>>) {
            for (i, c) in a.children.iter().enumerate() {
                let Some(c) = snap.by_id.get(c) else { continue };
                let last = i == a.children.len() - 1;
                let (branch, next) = if last { ("└─", format!("{prefix}   ")) } else { ("├─", format!("{prefix}│  ")) };
                let mut l = vec![
                    Span::styled(format!("{prefix}{branch}spawn \"{}\"─▶ ", labels.get(c.id.as_str()).copied().unwrap_or("")), faint()),
                    glyph(c.status),
                    Span::raw(format!(" {}", app.name(&c.id))),
                ];
                l.extend(annotate(&c.id));
                out.push(Line::from(l));
                rec(app, snap, c, &next, labels, annotate, out);
            }
        }
        rec(self, snap, root, "", &spawn_label, &annotate, &mut out);
        out
    }

    /// The Accounts section: per account, a bar for its 5-hour and one for
    /// its 7-day limit, in the session bars' style, or one line saying why
    /// there are none. Asking for the usage starts its fetching.
    fn accounts(&self, w: usize) -> Vec<Line<'static>> {
        let Some(limits) = &self.limits else { return Vec::new() };
        let usage = limits.usage();
        let mut head = vec![Span::styled("Accounts", bold())];
        if let Some(t) = usage.iter().map(|u| u.fetched_at).max().filter(|&t| t > 0) {
            head.push(Span::styled(format!("  fetched {}", clock(t)), faint()));
        }
        let mut out = vec![Line::from(head)];
        if usage.is_empty() {
            out.push(Line::styled("  no ~/.claude*/.credentials.json", faint()));
        }
        for u in &usage {
            out.extend(account_lines(u, w));
        }
        out.push(Line::raw(""));
        out
    }

    fn usage(&self, snap: &Snapshot, a: &Agent, w: usize) -> Vec<Line<'static>> {
        let root = snap.by_id.get(&a.session_id).unwrap_or(a);
        let total = snap.subtree_usage(&root.id);
        let mut out = vec![
            Line::from(vec![Span::styled(format!("Session {}", short_id(&root.id)), bold()), Span::raw(format!("  total {}", human_tokens(total.total())))]),
            Line::raw(""),
        ];
        let mut peak = 0;
        let mut by_model: BTreeMap<String, Usage> = BTreeMap::new();
        let mut members = Vec::new();
        snap.walk(&root.id, 0, &mut |x, depth| {
            peak = peak.max(x.usage.total());
            for (k, u) in &x.model_usage {
                let t = by_model.entry(k.clone()).or_default();
                *t = t.add(*u);
            }
            members.push((x, depth));
        });
        let bar_w = w.saturating_sub(30).max(4);
        for (x, depth) in members {
            let n = if peak > 0 { (bar_w as f64 * x.usage.total() as f64 / peak as f64) as usize } else { 0 };
            out.push(Line::from(vec![
                Span::raw(format!("{}  ", fit(&format!("{}{}", " ".repeat(depth), label(x)), 20))),
                Span::styled("█".repeat(n), Style::new().fg(Color::Indexed(63))),
                Span::styled("░".repeat(bar_w - n), faint()),
                Span::raw(format!(" {:>6}", human_tokens(x.usage.total()))),
            ]));
        }
        out.push(Line::raw(""));
        out.push(Line::styled("By model", bold()));
        let mut models: Vec<_> = by_model.into_iter().collect();
        models.sort_by(|a, b| b.1.total().cmp(&a.1.total()));
        for (k, u) in models {
            out.push(Line::from(vec![
                Span::raw(format!("  {} {:>7}  ", fit(&short_model(&k), 14), human_tokens(u.total()))),
                Span::styled(usage_detail(u), faint()),
            ]));
        }
        out.push(Line::raw(""));
        out.push(Line::from(vec![
            Span::styled("Session total ", bold()),
            Span::raw(format!("{}  ", human_tokens(total.total()))),
            Span::styled(usage_detail(total), faint()),
        ]));
        out
    }
}

/// One account's rows: `<name> 5h  ████░░  62%  resets 14:10` and
/// `        7d  ██░░░░  36%  resets Fri 22:00`, else `<name>  <why not>`.
fn account_lines(u: &AccountUsage, w: usize) -> Vec<Line<'static>> {
    let name = truncate(&u.name, 16);
    let why = match (&u.error, u.fetched_at) {
        (Some(e), _) => Some(e.clone()),
        (None, 0) => Some("fetching…".into()),
        _ => None,
    };
    if let Some(why) = why {
        return vec![Line::from(vec![Span::raw(format!("{}  ", fit(&name, 20))), Span::styled(why, faint())])];
    }
    // The label column of the session bars; room on the right for "100%  resets Fri 22:00".
    let bar_w = w.saturating_sub(45).max(4);
    let pad = " ".repeat(name.chars().count());
    vec![
        limit_line(&format!("{name} 5h"), u.five_hour, bar_w, hour_minute),
        limit_line(&format!("{pad} 7d"), u.seven_day, bar_w, weekday_time),
    ]
}

/// A bar of bar_w cells filled to the limit's percent, then the percent
/// and the reset time as `when` writes it.
fn limit_line(label: &str, l: Option<Limit>, bar_w: usize, when: fn(i64) -> String) -> Line<'static> {
    let pct = l.map_or(0.0, |l| l.pct.clamp(0.0, 100.0));
    let n = ((bar_w as f64 * pct / 100.0) as usize).min(bar_w);
    let mut spans = vec![
        Span::raw(format!("{}  ", fit(label, 20))),
        Span::styled("█".repeat(n), Style::new().fg(Color::Indexed(63))),
        Span::styled("░".repeat(bar_w - n), faint()),
        Span::raw(match l {
            Some(_) => format!(" {:>3}%", pct.round()),
            None => "    -".into(),
        }),
    ];
    if let Some(t) = l.map(|l| l.resets_at).filter(|&t| t > 0) {
        spans.push(Span::styled(format!("  resets {}", when(t)), faint()));
    }
    Line::from(spans)
}

/// Fri 22:00 in local time.
fn weekday_time(t: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let l = local(t);
    // 1970-01-01, day 0, was a Thursday.
    let day = DAYS[(days_from_civil(l.year, l.month, l.day) + 4).rem_euclid(7) as usize];
    format!("{day} {:02}:{:02}", l.hour, l.min)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::SEC;
    use ratatui::{Terminal, backend::TestBackend, crossterm::event::KeyEventKind};

    fn agent(id: &str, parent: &str, status: Status) -> Agent {
        Agent {
            id: id.into(),
            parent_id: parent.into(),
            title: format!("{id} title"),
            agent_type: "Explore".into(),
            model: "claude-opus-4-5".into(),
            project: "proj".into(),
            session_id: if parent.is_empty() { id.into() } else { "s1".into() },
            config: "work".into(),
            kind: if parent.is_empty() { Kind::Session } else { Kind::Subagent },
            status,
            last_activity: 100 * SEC,
            last_tool: "Bash: cargo test".into(),
            turns: 3,
            usage: Usage { input: 10, output: 20, ..Usage::default() },
            model_usage: [("claude-opus-4-5".to_string(), Usage { input: 10, output: 20, ..Usage::default() })].into(),
            events: Vec::new(),
            children: Vec::new(),
        }
    }

    fn snap() -> Snapshot {
        let mut s1 = agent("s1", "", Status::Active);
        s1.children = vec!["a1".into(), "a2".into()];
        let mut s = Snapshot { roots: vec!["s1".into(), "s2".into()], scanned_at: 110 * SEC, ..Snapshot::default() };
        for a in [s1, agent("a1", "s1", Status::Running), agent("a2", "s1", Status::Done), agent("s2", "", Status::Idle)] {
            s.by_id.insert(a.id.clone(), a);
        }
        s.edges.push(crate::scan::Edge { from: "s1".into(), to: "a1".into(), kind: EdgeKind::Spawn, at: 105 * SEC, label: "look".into() });
        s
    }

    fn press(app: &mut App, c: KeyCode) -> Action {
        app.key(KeyEvent { code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: ratatui::crossterm::event::KeyEventState::NONE })
    }

    /// The tree view's text on a 120×80 pane.
    fn screen(app: &mut App) -> String {
        let mut t = Terminal::new(TestBackend::new(120, 80)).unwrap();
        t.draw(|f| app.draw(f)).unwrap();
        let b = t.backend().buffer();
        (0..80).map(|y| (0..120).map(|x| b[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn office_keys_select_agents_of_every_session_and_switch_views() {
        let mut cast = Cast::load(std::path::Path::new("/nonexistent"), false, None);
        let mut app = App::new(0, 1);
        app.set_snapshot(snap());
        app.refresh_scene(110 * SEC, 0.0, &mut cast);
        // One room: both sessions are active, leads first.
        assert_eq!(app.scene.roster, ["s1", "s2", "a1", "a2"]);
        assert_eq!(app.focus, "s1");
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.focus, "s2");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, "a1");
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.focus, "s2");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.focus, "a1");

        assert_eq!(press(&mut app, KeyCode::Enter), Action::Switched);
        assert_eq!((app.view, app.selected.as_str()), (View::Tree, "a1"));
        let s = screen(&mut app);
        assert!(s.contains("1 Activity") && s.contains("now: Bash: cargo test"), "{s}");
        press(&mut app, KeyCode::Char('2'));
        assert!(screen(&mut app).contains("← spawn    from s1 session \"look\""));
        press(&mut app, KeyCode::Char('3'));
        assert!(screen(&mut app).contains("Session s1  total"));
        assert_eq!(press(&mut app, KeyCode::Char('v')), Action::Switched);
        assert_eq!(app.view, View::Office);
        app.refresh_scene(110 * SEC, 0.1, &mut cast);
        assert_eq!(app.focus, "a1");
        assert_eq!(press(&mut app, KeyCode::Char('q')), Action::Quit);
        assert_eq!(press(&mut app, KeyCode::Esc), Action::Quit);
    }

    /// The card's text, row by row.
    fn rows_of(b: &Buffer) -> Vec<String> {
        let a = b.area;
        (a.y..a.bottom()).map(|y| (a.x..a.right()).map(|x| b[(x, y)].symbol()).collect()).collect()
    }

    /// A snapshot where a1 ran tools, replied, was spawned by the lead and
    /// messaged it, and a2 handed back to it.
    fn busy_snap() -> Snapshot {
        use crate::scan::{Edge, Event};
        let mut s = snap();
        let a1 = s.by_id.get_mut("a1").unwrap();
        a1.events = vec![
            Event { at: 101 * SEC, kind: "tool_use", tool: "Bash".into(), summary: "Bash: cargo test".into() },
            Event { at: 102 * SEC, kind: "text", tool: String::new(), summary: "found it".into() },
        ];
        s.edges.push(Edge { from: "s1".into(), to: "a2".into(), kind: EdgeKind::Spawn, at: 106 * SEC, label: "fix".into() });
        s.edges.push(Edge { from: "a1".into(), to: "s1".into(), kind: EdgeKind::Message, at: 107 * SEC, label: "the bug is in parse".into() });
        s.edges.push(Edge { from: "a2".into(), to: "s1".into(), kind: EdgeKind::Handback, at: 108 * SEC, label: "handback".into() });
        s
    }

    /// There are no sessions to switch between: [ ] ← → do nothing.
    #[test]
    fn session_keys_do_nothing() {
        let mut cast = Cast::load(std::path::Path::new("/nonexistent"), false, None);
        let mut app = App::new(0, 1);
        app.set_snapshot(snap());
        app.refresh_scene(110 * SEC, 0.0, &mut cast);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('i'));
        let (focus, roster) = (app.focus.clone(), app.scene.roster.clone());
        for (i, k) in [KeyCode::Char('['), KeyCode::Char(']'), KeyCode::Left, KeyCode::Right].into_iter().enumerate() {
            assert_eq!(press(&mut app, k), Action::None, "{k:?}");
            app.refresh_scene(110 * SEC, 0.05 * (i + 1) as f64, &mut cast);
            assert_eq!((&app.focus, app.card, &app.scene.roster, app.view), (&focus, true, &roster, View::Office), "{k:?}");
        }
    }

    #[test]
    fn card_shows_who_what_and_with_whom() {
        let mut app = App::new(0, 1);
        app.set_snapshot(busy_snap());
        app.focus = "a1".into();
        let pane = Rect::new(0, 0, 140, 40);
        let card = card(app.card_lines(38), Rect::new(10, 5, 8, 6), pane);
        let text = rows_of(&card).join("\n");
        let a1 = format!("{} · a1 title", app.name_of("a1"));
        for want in [a1.as_str(), "role: subagent · Explore", "opus-4-5 · running", "last activity 10s ago", "now: Bash: cargo test", "did:", "with:"] {
            assert!(text.contains(want), "{want:?} not in\n{text}");
        }
        // Usage is the tree's: no tokens, turns or cache on the card.
        for unwanted in ["token", "turns", "in 10", "out 20", "cache"] {
            assert!(!text.contains(unwanted), "{unwanted:?} in\n{text}");
        }
        // Newest first, names resolved: the lead is "lead", others their name.
        let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s:?} not in\n{text}"));
        assert!(at(&format!("{} reply: found it", clock(102 * SEC))) < at(&format!("{} Bash: cargo test", clock(101 * SEC))), "{text}");
        assert!(at("message to lead 3s \"the bug is in parse\"") < at("spawned by lead 5s"), "{text}");
        assert!(rows_of(&card)[0].starts_with('┌') && rows_of(&card).last().unwrap().starts_with('└'), "{text}");

        app.focus = "s1".into();
        let text = app.card_lines(38).iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
        assert!(text.contains("role: session lead\nopus-4-5 · active\n"), "{text}");
        let (n1, n2) = (app.name_of("a1"), app.name_of("a2"));
        let want = [format!("handback from {n2} "), format!("message from {n1} "), format!("spawned {n2} "), format!("spawned {n1} ")];
        let order: Vec<usize> = want.iter().map(|s| text.find(s.as_str()).unwrap_or_else(|| panic!("{s:?} not in\n{text}"))).collect();
        assert!(order.is_sorted(), "newest first: {order:?}\n{text}");
        assert!(text.contains("did:\n(none)"), "{text}");
    }

    /// The card calls every agent by the file name of its character, without
    /// the extension and as written, never its id: in its first line, in
    /// what it did (ids cut short too) and in whom it dealt with. The name is
    /// the sprite it is drawn with, and agents not drawn are named too.
    #[test]
    fn card_names_agents_by_character_file_not_ids() {
        use crate::scan::Event;
        let (lead, sub, peer) = ("2ac51449-c1e2-4e4b-a850-045b77a0fca3", "a5582ca84087f834d", "a90e3abd17305b7ae");
        let mut s = Snapshot { roots: vec![lead.into()], scanned_at: 110 * SEC, ..Snapshot::default() };
        let mut a = agent(sub, lead, Status::Running);
        a.title = String::new();
        a.last_tool = format!("SendMessage: to {}…", &peer[..10]);
        a.events = vec![Event { at: 101 * SEC, kind: "tool_use", tool: "SendMessage".into(), summary: format!("SendMessage: to {lead}") }];
        for x in [agent(lead, "", Status::Active), a, agent(peer, lead, Status::Idle)] {
            s.by_id.insert(x.id.clone(), x);
        }
        let dir = std::env::temp_dir().join(format!("office-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let px = image::RgbaImage::from_pixel(2, 3, [1, 2, 3, 255].into());
        for f in ["Luffy.png", "nami.png", "session.png"] {
            px.save(dir.join(f)).unwrap();
        }
        let mut cast = Cast::load(&dir, true, None);
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::new(0, 1);
        app.set_snapshot(s);
        app.refresh_scene(110 * SEC, 0.0, &mut cast);
        let names: Vec<String> = [lead, sub, peer].iter().map(|id| app.name_of(id).to_string()).collect();
        assert_eq!(names[0], "session", "the lead takes the file named after its type");
        assert!(["Luffy", "nami"].contains(&names[1].as_str()) && ["Luffy", "nami"].contains(&names[2].as_str()), "{names:?}");
        for a in &app.scene.actors {
            if a.id != actors::PET {
                assert_eq!(cast.name(a.sprite), app.name_of(&a.id), "the name is the sprite drawn");
            }
        }

        app.focus = sub.into();
        let text = app.card_lines(38).iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
        assert!(!text.contains(&lead[..8]) && !text.contains(&sub[..8]) && !text.contains(&peer[..8]), "an id on the card:\n{text}");
        assert!(text.starts_with(&format!("● {}\n", names[1])), "{text}");
        assert!(text.contains(&format!("now: SendMessage: to {}", names[2])) && text.contains("SendMessage: to lead"), "{text}");
        app.focus = lead.into();
        let text = app.card_lines(38).iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
        assert!(text.lines().next().is_some_and(|l| l.contains(" session · ")), "the lead's name leads its card:\n{text}");
    }

    /// A short pane keeps the card's sections, with fewer lines in each.
    #[test]
    fn card_fits_a_short_pane() {
        let mut s = busy_snap();
        let a1 = s.by_id.get_mut("a1").unwrap();
        a1.events = (0..20).map(|i| crate::scan::Event { at: i * SEC, kind: "tool_use", tool: "Read".into(), summary: format!("Read: f{i}") }).collect();
        let mut app = App::new(0, 1);
        app.set_snapshot(s);
        app.focus = "a1".into();
        let lines = app.card_lines(12);
        let text = lines.iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
        assert_eq!(lines.len(), 12, "{text}");
        assert!(text.contains("Read: f19") && !text.contains("Read: f15") && text.contains("with:\nmessage to lead"), "{text}");
        assert_eq!(app.card_lines(40).iter().filter(|l| l.to_string().starts_with("Read: ") || l.to_string().contains(" Read: ")).count(), 3);
    }

    #[test]
    fn card_goes_beside_its_agent_inside_the_pane() {
        let pane = Rect::new(0, 0, 140, 40);
        let size = (30, 12);
        // Room on the right: just right of the agent, top aligned.
        assert_eq!(place(size, Rect::new(50, 10, 8, 6), pane), Rect::new(59, 10, 30, 12));
        // Near the bottom: lifted to stay inside.
        assert_eq!(place(size, Rect::new(50, 35, 8, 5), pane), Rect::new(59, 28, 30, 12));
        // No room on the right: left of it.
        assert_eq!(place(size, Rect::new(120, 10, 8, 6), pane), Rect::new(89, 10, 30, 12));
        // No room either side: below it, then above it.
        let narrow = Rect::new(0, 0, 40, 40);
        assert_eq!(place(size, Rect::new(16, 4, 8, 6), narrow), Rect::new(10, 10, 30, 12));
        assert_eq!(place(size, Rect::new(16, 30, 8, 6), narrow), Rect::new(10, 18, 30, 12));
        // Nowhere beside: at the right edge. Too big: cut to the pane.
        let tiny = Rect::new(0, 0, 32, 14);
        assert_eq!(place(size, Rect::new(10, 4, 8, 6), tiny), Rect::new(2, 2, 30, 12));
        assert_eq!(place((60, 50), Rect::new(10, 4, 8, 6), tiny), tiny);
        for x in (0..140).step_by(7) {
            for y in (0..40).step_by(3) {
                let r = place(size, Rect::new(x, y, 8, 6), pane);
                assert!(r.right() <= pane.right() && r.bottom() <= pane.bottom(), "{r:?} for ({x}, {y})");
            }
        }
    }

    #[test]
    fn click_i_and_esc_open_and_close_the_card() {
        let mut cast = Cast::load(std::path::Path::new("/nonexistent"), false, None);
        let mut app = App::new(0, 1);
        app.set_snapshot(snap());
        app.refresh_scene(110 * SEC, 0.0, &mut cast);
        app.click(Some("a2".into()));
        assert_eq!((app.focus.as_str(), app.card), ("a2", true));
        app.click(None);
        assert_eq!((app.focus.as_str(), app.card), ("a2", false));
        assert_eq!(press(&mut app, KeyCode::Char('i')), Action::None);
        assert!(app.card);
        // The card follows the selection.
        press(&mut app, KeyCode::Char('k'));
        assert_eq!((app.focus.as_str(), app.card), ("a1", true));
        // Esc closes an open card, and quits only when none is.
        assert_eq!(press(&mut app, KeyCode::Esc), Action::None);
        assert!(!app.card);
        assert_eq!(press(&mut app, KeyCode::Esc), Action::Quit);
        press(&mut app, KeyCode::Char('i'));
        press(&mut app, KeyCode::Char('i'));
        assert!(!app.card, "i toggles");
        // The tree closes it.
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(press(&mut app, KeyCode::Char('v')), Action::Switched);
        assert!(!app.card);
        // So does its agent leaving the room; the first one is selected.
        app.view = View::Office;
        (app.focus, app.card) = ("a2".into(), true);
        app.snap.as_mut().unwrap().by_id.get_mut("a2").unwrap().last_activity = 110 * SEC - actors::LEAVE_AFTER - 1;
        app.refresh_scene(110 * SEC, 0.1, &mut cast);
        assert_eq!((app.focus.as_str(), app.card), ("s1", false));
    }

    #[test]
    fn limit_bars_fill_to_the_percent() {
        let bar = |pct: f64| limit_line("work 5h", Some(Limit { pct, resets_at: 0 }), 10, hour_minute).to_string();
        assert_eq!(bar(0.0), format!("{}  ░░░░░░░░░░   0%", fit("work 5h", 20)));
        assert_eq!(bar(62.0), format!("{}  ██████░░░░  62%", fit("work 5h", 20)));
        assert_eq!(bar(100.0), format!("{}  ██████████ 100%", fit("work 5h", 20)));
        assert_eq!(bar(140.0), bar(100.0));
        assert_eq!(limit_line("x", None, 4, hour_minute).to_string(), format!("{}  ░░░░    -", fit("x", 20)));
        let t = crate::scan::parse_rfc3339("2026-11-21T22:00:00Z").unwrap();
        let l = limit_line("x", Some(Limit { pct: 36.0, resets_at: t }), 4, weekday_time).to_string();
        assert!(l.ends_with(&format!(" 36%  resets {}", weekday_time(t))), "{l}");
        assert!(["Sat", "Sun"].iter().any(|d| weekday_time(t).starts_with(d)), "a Saturday in UTC: {}", weekday_time(t));
    }

    #[test]
    fn usage_tab_lists_accounts() {
        use crate::limits::Account;
        let u = AccountUsage {
            name: "personal".into(),
            five_hour: Some(Limit { pct: 62.0, resets_at: 100 * SEC }),
            seven_day: Some(Limit { pct: 36.0, resets_at: 200 * SEC }),
            fetched_at: 90 * SEC,
            error: None,
        };
        let rows: Vec<String> = account_lines(&u, 68).iter().map(Line::to_string).collect();
        assert!(rows[0].starts_with("personal 5h  ") && rows[0].contains(&format!(" 62%  resets {}", hour_minute(100 * SEC))), "{rows:?}");
        assert!(rows[1].starts_with("         7d  ") && rows[1].contains(&format!(" 36%  resets {}", weekday_time(200 * SEC))), "{rows:?}");
        assert!(rows.iter().all(|r| r.chars().count() <= 68), "{rows:?}");
        let e = AccountUsage { name: "work".into(), error: Some("HTTP 429".into()), ..AccountUsage::default() };
        assert_eq!(account_lines(&e, 68).iter().map(Line::to_string).collect::<Vec<_>>(), [format!("{}  HTTP 429", fit("work", 20))]);

        // Offline: on the Usage tab, above the session's bars; never on the others.
        let mut app = App::new(0, 1);
        app.view = View::Tree;
        app.set_snapshot(snap());
        let acct = |name: &str| Account { name: name.into(), dir: "/nonexistent".into() };
        app.limits = Some(Limits::new(vec![acct("default"), acct("work")], true));
        let s = screen(&mut app);
        assert!(!s.contains("Accounts"), "{s}");
        press(&mut app, KeyCode::Char('3'));
        let s = screen(&mut app);
        let at = |t: &str| s.find(t).unwrap_or_else(|| panic!("{t:?} not in\n{s}"));
        let (default, work) = (format!("{}  offline", fit("default", 20)), format!("{}  offline", fit("work", 20)));
        assert!(at("Accounts") < at(&default) && at(&default) < at(&work) && at(&work) < at("Session s1  total"), "{s}");
    }

    #[test]
    fn tree_collapses_and_moves() {
        let mut app = App::new(0, 1);
        app.view = View::Tree;
        app.set_snapshot(snap());
        assert_eq!(app.rows().len(), 4);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.rows().len(), 2, "s1 collapsed");
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected, "s2");
        let s = screen(&mut app);
        assert!(s.contains("▸ ◐ s1 s1 title [work]"), "{s}");
    }
}
