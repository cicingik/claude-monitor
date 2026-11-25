//! The keys of the office view and the card of its selected agent, and the
//! tree view with its Activity, Interactions and Usage tabs; the Usage tab
//! starts with every account's 5-hour and 7-day limits.
//! The office view is the picture alone; its only text is
//! the card, while it is open.

use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
};

use crate::{
    actors::{self, Scene, Walker},
    bridge::{self, Launch},
    cast::Cast,
    limits::Limits,
    scan::{Agent, Kind, Snapshot, Status},
};

mod card;
mod draw;
mod new;
mod panel;
mod select;

pub use card::card;

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
    RunHere(Here), // hand the terminal to `claude --resume` until it exits
}

/// A session to resume or start in office's own terminal, for what a
/// background session cannot do: ask whether to trust its folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Here {
    pub launch: Launch,
    pub cwd: String,
    pub config_dir: PathBuf,
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
    // The session panel under the tree, for the session of the selected row.
    pub bridge: PathBuf, // bridge::dir(), a temporary one in tests
    pub claude: PathBuf, // the claude program sessions are resumed with; in tests, none, or a fake
    log_scroll: usize,     // how many lines the conversation is scrolled up from its newest
    log_max: Cell<usize>,  // how far it can be, as last drawn
    log_for: String,       // the session the scroll is for: another starts at the newest                           // the command picked in the menu of those matching the input
    note: String,     // what the last prompt or answer did
    live: bool, // a running Claude Code process holds the session
    resuming: Option<Resuming>,
    selection: Option<select::Selection>, // text dragged over in the tree view
    panes: Cell<[Rect; 3]>, // the insides of the tree, prompt and detail panes, as last drawn
    pub copied: Option<String>, // selected text for main to put on the clipboard
    here_for: Option<Here>, // the session whose background resume needs a terminal: o runs it here
    picker: Option<new::Picker>, // the new-session picker over the tree, while open
    select_started: Option<String>, // a session office started: selected once a scan shows it
    term: Option<crate::embed::Embed>, // Claude Code's own terminal in the panel: it has every key while open
    term_for: Option<(String, std::time::Instant)>, // a session resuming to open there, and since when
}

/// A `claude --bg` until it returns, and what explains its result.
struct Resuming {
    launch: Launch,
    child: std::process::Child,
    cwd: String,
    config_dir: PathBuf,
}

/// The fewest rows of the session panel, borders included; it takes two thirds
/// of the left column when there is room.
const PANEL_H: u16 = 9;

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
            bridge: bridge::dir(),
            claude: (if cfg!(test) { "office-test-no-claude" } else { "claude" }).into(),
            log_scroll: 0,
            log_max: Cell::new(0),
            log_for: String::new(),
            note: String::new(),
            live: false,
            term: None,
            term_for: None,
            resuming: None,
            selection: None,
            panes: Cell::new([Rect::default(); 3]),
            copied: None,
            here_for: None,
            picker: None,
            select_started: None,
        }
    }

    /// The mouse wheel: it scrolls the conversation while the panel has the
    /// keys, else it moves the tree's cursor or the office's focus as ↑/↓ do.
    pub fn wheel(&mut self, up: bool) -> Action {
        if let Some(t) = self.term.as_mut().filter(|_| self.view == View::Tree) {
            t.wheel(up);
            return Action::None;
        }
        self.key(KeyEvent::from(if up { KeyCode::Up } else { KeyCode::Down }))
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        let started = self.select_started.as_ref().and_then(|short| snap.roots.iter().find(|id| id.starts_with(short.as_str())).cloned());
        self.snap = Some(snap);
        if let Some(id) = started {
            // Claude Code's terminal opens on it once it is registered.
            (self.selected, self.select_started, self.term_for) = (id.clone(), None, Some((id, std::time::Instant::now())));
        }
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

    /// A paste, whole: into the picker's input (a folder holds no line
    /// break, so its breaks go), else into the prompt while the panel has
    /// the keys, line breaks kept. Elsewhere it is dropped: pasted text
    /// never runs the tree's keys.
    pub fn paste(&mut self, text: &str) {
        self.selection = None;
        if self.view != View::Tree {
            return;
        }
        if let Some(t) = self.term.as_mut() {
            t.paste(text);
        } else if let Some(p) = self.picker.as_mut() {
            p.paste(&text.replace(['\r', '\n'], ""));
        }
    }

    /// Handles a key press.
    pub fn key(&mut self, k: KeyEvent) -> Action {
        self.selection = None;
        // Claude Code's terminal takes every key, ctrl+c too, but esc, which detaches.
        if let Some(t) = self.term.as_mut().filter(|_| self.view == View::Tree) {
            if k.code == KeyCode::Esc {
                self.term = None; // ends the attach, never the session
                self.note = "detached: the session runs on in the background".into();
            } else {
                t.key(k);
            }
            return Action::None;
        }
        let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
        if self.picker.is_some() && self.view == View::Tree && !ctrl_c {
            return self.picker_key(k);
        }
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
            // The tree, on the selected agent, with its session's Claude Code terminal open.
            KeyCode::Char('p') => {
                let switched = self.office_key(KeyCode::Enter);
                return match self.open_term() {
                    Action::None => switched,
                    run => run,
                };
            }
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
            KeyCode::Char('p') => return self.open_term(),
            KeyCode::PageUp => self.scroll_log(self.page() as isize),
            KeyCode::PageDown => self.scroll_log(-(self.page() as isize)),
            KeyCode::Char('n') => self.open_picker(),
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

    /// Rows of the tree's box and of the panel's under it, borders included.
    fn left_heights(&self) -> (u16, u16) {
        let body = self.height.saturating_sub(2).max(4);
        let panel = (body * 2 / 3).max(PANEL_H).min(body.saturating_sub(5));
        (body - panel, panel)
    }

    fn tree_height(&self) -> usize {
        usize::from(self.left_heights().0.max(3)) - 2
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{card::place, draw::{account_lines, limit_line, weekday_time}};
    use crate::{
        format::{clock, fit, hour_minute},
        limits::{AccountUsage, Limit},
        scan::{EdgeKind, SEC, Usage},
    };
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, crossterm::event::KeyEventKind, text::Line};

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
            last_text: String::new(),
            config_dir: Default::default(),
            cwd: String::new(),
            asking: String::new(),
            turns: 3,
            usage: Usage { input: 10, output: 20, ..Usage::default() },
            model_usage: [("claude-opus-4-5".to_string(), Usage { input: 10, output: 20, ..Usage::default() })].into(),
            events: Vec::new(),
            children: Vec::new(),
            live: false,
            continued: false,
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
            Event { at: 101 * SEC, kind: "tool_use", tool: "Bash".into(), summary: "Bash: cargo test".into(), ..Default::default() },
            Event { at: 102 * SEC, kind: "text", tool: String::new(), summary: "found it".into(), ..Default::default() },
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
        a.events = vec![Event { at: 101 * SEC, kind: "tool_use", tool: "SendMessage".into(), summary: format!("SendMessage: to {lead}"), ..Default::default() }];
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

    /// The panel shows what the session asks, from the top, else the end of
    /// its last reply, in the rows it has, with long lines wrapped at spaces.
    #[test]
    fn panel_shows_what_the_session_asks_or_last_said() {
        let mut snap = busy_snap();
        let s1 = snap.by_id.get_mut("s1").unwrap();
        s1.last_text = format!("{}\nShould I also delete the old build directory before rebuilding?", "line\n".repeat(30));
        let mut app = App::new(0, 1);
        app.set_snapshot(snap.clone());
        (app.selected, app.view) = ("a1".into(), View::Tree);
        let text = |app: &App| app.panel_lines(40, 12).iter().map(Line::to_string).collect::<Vec<_>>();
        let t = text(&app);
        assert_eq!(t.len(), 12, "{t:#?}");
        assert!(t[1].starts_with("↑ ") && t[1].ends_with("more · pgup"), "older lines are above: {t:#?}");
        let end = t.iter().position(|l| l.starts_with("  Should I")).expect("the reply's end shows, at the bottom");
        assert_eq!(t[end..], ["  Should I also delete the old build", "  directory before rebuilding?"], "{t:#?}");

        // Page up in the tree shows older lines, page down the newest again.
        app.key(KeyEvent::from(KeyCode::PageUp));
        let t = text(&app);
        assert!(!t.iter().any(|l| l.starts_with("  Should I")) && t.iter().any(|l| l.starts_with("↓ ") && l.ends_with("newer · pgdn")), "{t:#?}");
        app.key(KeyEvent::from(KeyCode::PageDown));
        assert!(text(&app).iter().any(|l| l.starts_with("  Should I")));

        snap.by_id.get_mut("s1").unwrap().asking = "Which approach?\n  1. Fast — skip the cache\n  2. Safe".into();
        app.set_snapshot(snap);
        let t = text(&app);
        let at = t.iter().position(|l| l == "asks you · p to answer:").expect("the question shows");
        assert_eq!(t[at + 1..], ["Which approach?", "  1. Fast — skip the cache", "  2. Safe"], "{t:#?}");
    }

    /// The conversation reads as in Claude Code: prompts after `>`, replies
    /// after `●` whole and wrapped, tool calls on one line.
    #[test]
    fn panel_shows_the_conversation() {
        let mut snap = busy_snap();
        snap.by_id.get_mut("s1").unwrap().events = vec![
            crate::scan::Event { kind: "prompt", summary: "fix the build".into(), text: "fix the build".into(), ..Default::default() },
            crate::scan::Event { kind: "tool_use", tool: "Bash".into(), summary: "Bash: cargo build".into(), ..Default::default() },
            crate::scan::Event { kind: "text", summary: "Fixed: a missing import".into(), text: "Fixed: a missing import in main.rs, and the tests pass again.".into(), ..Default::default() },
        ];
        let mut app = App::new(0, 1);
        app.set_snapshot(snap);
        app.selected = "s1".into();
        let t = app.panel_lines(40, 14).iter().map(Line::to_string).collect::<Vec<_>>();
        let at = t.iter().position(|l| l == "> fix the build").expect("the prompt");
        assert_eq!(t[at + 1..at + 5], ["● Bash: cargo build", "", "● Fixed: a missing import in main.rs,", "  and the tests pass again."], "{t:#?}");
    }

    /// A background resume refused for trust offers to run it here: o (or p)
    /// hands office's terminal to that session.
    #[test]
    fn an_untrusted_resume_offers_to_run_it_here() {
        let mut app = App::new(0, 1);
        app.view = View::Tree;
        app.set_snapshot(busy_snap());
        app.selected = "s1".into();
        let child = std::process::Command::new("sh")
            .args(["-c", "echo 'Workspace not trusted. Run `claude` in /tmp once and accept the trust prompt, then retry.' >&2; exit 1"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        app.resuming = Some(Resuming { launch: Launch::Resume("s1".into()), child, cwd: "/tmp".into(), config_dir: "/cfg".into() });
        while app.resuming.is_some() {
            app.poll_resume();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(app.note.contains("/tmp is not trusted") && app.note.contains("Press p to run it here"), "{}", app.note);
        let want = Here { launch: Launch::Resume("s1".into()), cwd: "/tmp".into(), config_dir: "/cfg".into() };
        assert_eq!(press(&mut app, KeyCode::Char('p')), Action::RunHere(want.clone()));
        app.ran_here(&want, Ok(std::process::Command::new("true").status().unwrap()));
        assert!(app.note.starts_with("back from s1"), "{}", app.note);
        assert!(app.here_for.is_none(), "offered once");
    }

    /// p on a session no process holds resumes it in the background, and
    /// its terminal opens once Claude Code registers it.
    #[test]
    fn p_on_a_session_not_running_resumes_it_to_open_there() {
        let root = std::env::temp_dir().join(format!("office-autoresume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (fake, said) = (root.join("claude"), root.join("args"));
        std::fs::write(&fake, format!("#!/bin/sh\necho \"$@\" > {}\n", said.display())).unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut app = App::new(0, 1);
        app.claude = fake;
        let mut snap = busy_snap();
        snap.by_id.get_mut("s1").unwrap().config_dir = root.join("cfg");
        app.set_snapshot(snap);
        app.focus = "s1".into();
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(app.note, "resuming it in the background…");
        while app.resuming.is_some() {
            app.poll_resume();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_to_string(&said).unwrap().trim(), "--bg --resume s1");
        app.poll_bridge();
        assert!(app.term_for.as_ref().is_some_and(|(id, _)| id == "s1") && app.term.is_none(), "it opens once registered");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// n opens the picker over the tree: enter takes the folder a session
    /// ran in, the first prompt starts `claude --bg -- <prompt>` there, and
    /// the new session is selected once a scan shows it, its terminal to open.
    #[test]
    fn n_starts_a_new_session_in_a_picked_folder() {
        let root = std::env::temp_dir().join(format!("office-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let proj = root.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let (fake, said) = (root.join("claude"), root.join("args"));
        std::fs::write(&fake, format!("#!/bin/sh\necho \"$@\" > {}\necho 'backgrounded · abcd1234 (idle — send a prompt to start)'\n", said.display())).unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut app = App::new(0, 1);
        (app.claude, app.view) = (fake, View::Tree);
        let mut snap = busy_snap();
        let s1 = snap.by_id.get_mut("s1").unwrap();
        (s1.cwd, s1.config_dir) = (proj.to_string_lossy().into_owned(), root.join("cfg"));
        app.set_snapshot(snap.clone());
        app.selected = "s1".into();
        press(&mut app, KeyCode::Char('n'));
        let text = app.picker_lines(60, 20).iter().map(Line::to_string).collect::<Vec<_>>();
        assert!(text[0] == "new session" && text.iter().any(|l| l.trim() == crate::bridge::tilde(&proj.to_string_lossy())), "{text:#?}");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert!(app.note.starts_with("type the first prompt"), "{}", app.note);
        for c in "logs".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.picker.is_none() && app.note.starts_with("starting a session in"), "{}", app.note);
        while app.resuming.is_some() {
            app.poll_resume();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_to_string(&said).unwrap().trim(), "--bg -- logs", "the prompt is never a command of claude's");
        assert!(app.note.starts_with("started abcd1234"), "{}", app.note);
        let new = agent("abcd1234-0000@work", "", Status::Running);
        snap.roots.insert(0, new.id.clone());
        snap.by_id.insert(new.id.clone(), new);
        app.set_snapshot(snap);
        assert_eq!(app.selected, "abcd1234-0000@work");
        assert!(app.term_for.as_ref().is_some_and(|(id, _)| id == "abcd1234-0000@work"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A new session in a folder not trusted yet offers to start it here.
    #[test]
    fn an_untrusted_new_session_offers_to_start_it_here() {
        let mut app = App::new(0, 1);
        app.view = View::Tree;
        app.set_snapshot(busy_snap());
        let child = std::process::Command::new("sh")
            .args(["-c", "echo 'Workspace not trusted. Run `claude` in /tmp once and accept the trust prompt, then retry.' >&2; exit 1"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let launch = Launch::New("hi".into());
        app.resuming = Some(Resuming { launch: launch.clone(), child, cwd: "/tmp".into(), config_dir: "/cfg".into() });
        while app.resuming.is_some() {
            app.poll_resume();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(app.note.starts_with("not started: /tmp is not trusted"), "{}", app.note);
        let want = Here { launch, cwd: "/tmp".into(), config_dir: "/cfg".into() };
        assert_eq!(press(&mut app, KeyCode::Char('p')), Action::RunHere(want));
    }

    /// A long note wraps over several rows instead of being cut to one.
    #[test]
    fn panel_note_wraps_whole() {
        let mut app = App::new(0, 1);
        app.set_snapshot(busy_snap());
        app.selected = "s1".into();
        app.note = "not resumed: ~/dev/api is not trusted for this account yet. Run this once in a terminal and accept the trust prompt, then press p again: cd ~/dev/api && claude".into();
        let t = app.panel_lines(40, 20).iter().map(Line::to_string).collect::<Vec<_>>();
        let at = t.iter().position(|l| l.starts_with("not resumed:")).expect("the note");
        assert!(t[at..].join(" ").ends_with("cd ~/dev/api && claude") && !t[at..].join(" ").contains('…'), "{t:#?}");
    }

    /// A session row is `[account](directory) title`; no row shows the
    /// model or tokens; subagents keep their type.
    #[test]
    fn tree_rows_name_account_directory_and_title() {
        let mut snap = busy_snap();
        let s1 = snap.by_id.get_mut("s1").unwrap();
        (s1.config, s1.project, s1.title) = ("work".into(), "proj".into(), "Fix the bug".into());
        let mut app = App::new(0, 1);
        app.set_snapshot(snap);
        let rows: Vec<String> = app.tree_lines(80).iter().map(Line::to_string).collect();
        let s = rows.iter().find(|r| r.contains("[work](proj) Fix the bug")).unwrap_or_else(|| panic!("{rows:#?}"));
        assert!(!s.contains("opus") && !s.contains("session"), "{s}");
        assert!(rows.iter().all(|r| !r.contains("opus") && !r.contains("tok")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("Explore")), "subagents keep their type: {rows:#?}");
    }

    /// A drag in the tree view selects text inside the pane it starts in,
    /// highlighted, and the release copies it; a click alone copies nothing.
    #[test]
    fn a_drag_in_the_tree_view_copies_the_text_of_one_pane() {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let mut app = App::new(0, 1);
        app.set_snapshot(busy_snap());
        app.view = View::Tree;
        let s = screen(&mut app);
        let rows: Vec<Vec<char>> = s.lines().map(|l| l.chars().collect()).collect();
        let word: Vec<char> = "Recent events".chars().collect();
        let (y, x) = rows.iter().enumerate().find_map(|(y, r)| r.windows(word.len()).position(|w| w == word).map(|x| (y, x))).expect("Recent events on screen");
        let (x, y) = (x as u16, y as u16);

        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), x + 5, y);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), x + 12, y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), x + 12, y);
        let mut t = Terminal::new(TestBackend::new(120, 80)).unwrap();
        t.draw(|f| app.draw(f)).unwrap();
        assert!(t.backend().buffer()[(x + 3, y)].modifier.contains(Modifier::REVERSED), "the selection shows");
        assert_eq!(app.copied.take().as_deref(), Some("Recent events"));
        assert!(app.note.starts_with("copied 13 chars"), "{}", app.note);

        // Dragged out over the tree to the left, it stops at its pane's edge.
        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), 0, y + 1);
        app.mouse(MouseEventKind::Up(MouseButton::Left), 0, y + 1);
        t.draw(|f| app.draw(f)).unwrap();
        let text = app.copied.take().expect("copied");
        assert!(text.starts_with("Recent events") && text.split('\n').count() == 2 && !text.contains('│'), "{text:?}");

        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
        t.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.copied, None, "a click selects nothing");
        assert!(!t.backend().buffer()[(x, y)].modifier.contains(Modifier::REVERSED));
    }

    /// p on an agent in the office opens the tree on it, with its session's
    /// background session in Claude Code's terminal: every key but esc goes
    /// there; when the attach ends, or on esc, the tree has the keys again.
    #[test]
    fn p_opens_claude_codes_terminal_for_the_agents_session() {
        let root = std::env::temp_dir().join(format!("office-term-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("cfg").join("sessions")).unwrap();
        let me = std::process::id();
        let held = serde_json::json!({ "pid": me, "sessionId": "s1", "kind": "bg", "jobId": "job1" });
        std::fs::write(root.join("cfg").join("sessions").join("1.json"), held.to_string()).unwrap();
        let fake = root.join("claude");
        std::fs::write(&fake, "#!/bin/sh\necho \"attached to $2\"\nread line\necho \"got $line\"\n").unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut app = App::new(0, 1);
        app.claude = fake;
        let mut snap = busy_snap();
        snap.by_id.get_mut("s1").unwrap().config_dir = root.join("cfg");
        app.set_snapshot(snap);
        app.focus = "a1".into();
        assert_eq!(press(&mut app, KeyCode::Char('p')), Action::Switched);
        assert_eq!((app.view, app.selected.as_str()), (View::Tree, "a1"));
        assert_eq!(app.term.as_ref().map(|t| t.session.as_str()), Some("s1"), "a subagent's terminal is its session's");
        let wait = |app: &mut App, what: &str| {
            for _ in 0..200 {
                if app.term.as_ref().is_some_and(|t| t.said().contains(what)) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("never showed {what}: {:?}", app.term.as_ref().map(|t| t.said()));
        };
        wait(&mut app, "attached to job1");
        for c in "quit".chars() {
            assert_eq!(press(&mut app, KeyCode::Char(c)), Action::None, "keys go to claude, q too");
        }
        press(&mut app, KeyCode::Enter);
        for _ in 0..200 {
            app.poll_bridge();
            if app.term.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.term.is_none() && app.note.contains("got quit"), "the attach ended: {}", app.note);
        press(&mut app, KeyCode::Char('p'));
        wait(&mut app, "attached to job1");
        assert_eq!(press(&mut app, KeyCode::Esc), Action::None, "esc detaches, never quits");
        assert!(app.term.is_none() && app.note.starts_with("detached"), "{}", app.note);
        assert_eq!(press(&mut app, KeyCode::Char('q')), Action::Quit, "the tree's keys again");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A short pane keeps the card's sections, with fewer lines in each.
    #[test]
    fn card_fits_a_short_pane() {
        let mut s = busy_snap();
        let a1 = s.by_id.get_mut("a1").unwrap();
        a1.events = (0..20).map(|i| crate::scan::Event { at: i * SEC, kind: "tool_use", tool: "Read".into(), summary: format!("Read: f{i}"), ..Default::default() }).collect();
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
        assert!(s.contains("▸ ◐ [work](proj) s1 title"), "{s}");
    }
}
