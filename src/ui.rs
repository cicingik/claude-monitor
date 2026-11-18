//! The keys of the office view and the card of its selected agent, and the
//! tree view with its Activity, Interactions and Usage tabs.
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
    format::{age, agent_type, clock, fit, hour_minute, human_tokens, label, short_duration, short_id, short_model, usage_detail},
    scan::{Agent, EdgeKind, Kind, Snapshot, Status, Usage, truncate},
};

const TABS: [&str; 3] = ["Activity", "Interactions", "Usage"];

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
        Status::Active => 220,
        Status::Idle => 244,
        Status::Done => 39,
    })
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
        let picks = cast.pick(sc.actors.iter().map(|a| (a.id.as_str(), a.kind.as_str())));
        for (a, p) in sc.actors.iter_mut().zip(picks) {
            (a.sprite, a.selected) = (p, a.id == self.focus);
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
                let since = if self.since > 0 { short_duration(self.since) } else { "all".into() };
                vec![
                    Span::styled("claude-monitor", header_style()),
                    Span::raw(format!(
                        "  sessions {} · agents {} · running {running}  │  scanned {}  │  since {since}",
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
        if a.status == Status::Running {
            out.push(Line::styled(format!("now: {}", a.last_tool), Style::new().fg(status_color(Status::Running)).add_modifier(Modifier::BOLD)));
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

    /// A short name for an agent on the card.
    fn who(&self, id: &str) -> String {
        match self.agent(id) {
            Some(a) if a.kind == Kind::Session => "lead".into(),
            Some(a) if !a.title.is_empty() => truncate(&a.title, 18),
            _ => short_id(id).into(),
        }
    }

    /// The selected agent's card in at most `rows` lines: who it is, what it
    /// did and whom it dealt with, newest first, up to 8 of each.
    pub fn card_lines(&self, rows: usize) -> Vec<Line<'static>> {
        let (Some(snap), Some(a)) = (&self.snap, self.agent(&self.focus)) else { return vec![Line::styled("nothing selected", faint())] };
        let now = snap.scanned_at;
        let mut out = vec![
            Line::from(vec![glyph(a.status), Span::raw(" "), Span::styled(label(a), bold())]),
            Line::raw(format!("{} · {} · {}", agent_type(a), short_model(&a.model), a.status.name())),
            Line::raw(format!("tokens {} · turns {} · last activity {} ago", human_tokens(a.usage.total()), a.turns, age(now, a.last_activity))),
            Line::styled(usage_detail(a.usage), faint()),
        ];
        if a.status == Status::Running {
            out.push(Line::styled(format!("now: {}", a.last_tool), Style::new().fg(status_color(Status::Running)).add_modifier(Modifier::BOLD)));
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
        let (mut nd, mut nw) = (did.len().min(8), with.len().min(8));
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
    let w = u16::try_from(lines.iter().map(Line::width).max().unwrap_or(0) + 4).unwrap_or(u16::MAX).clamp(24, 56);
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
        for want in ["a1 title", "Explore · opus-4-5 · running", "tokens 30 · turns 3 · last activity 10s ago", "now: Bash: cargo test", "did:", "with:"] {
            assert!(text.contains(want), "{want:?} not in\n{text}");
        }
        // Newest first, names resolved: the lead is "lead", others their title.
        let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s:?} not in\n{text}"));
        assert!(at(&format!("{} reply: found it", clock(102 * SEC))) < at(&format!("{} Bash: cargo test", clock(101 * SEC))), "{text}");
        assert!(at("message to lead 3s \"the bug is in parse\"") < at("spawned by lead 5s"), "{text}");
        assert!(rows_of(&card)[0].starts_with('┌') && rows_of(&card).last().unwrap().starts_with('└'), "{text}");

        app.focus = "s1".into();
        let text = app.card_lines(38).iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
        let order: Vec<usize> = ["handback from a2 title", "message from a1 title", "spawned a2 title", "spawned a1 title"].iter().map(|s| text.find(s).unwrap_or_else(|| panic!("{s:?} not in\n{text}"))).collect();
        assert!(order.is_sorted(), "newest first: {order:?}\n{text}");
        assert!(text.contains("did:\n(none)"), "{text}");
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
        assert_eq!(app.card_lines(40).iter().filter(|l| l.to_string().starts_with("Read: ") || l.to_string().contains(" Read: ")).count(), 8);
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
