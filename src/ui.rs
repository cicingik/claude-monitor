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
    bridge::{self, Ask, Launch},
    cast::Cast,
    commands::Command,
    edit::LineEdit,
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
    // The prompt panel under the tree, for the session of the selected row.
    pub bridge: PathBuf, // bridge::dir(), a temporary one in tests
    pub claude: PathBuf, // the claude program sessions are resumed with; in tests, none, or a fake
    pub hooks: bool,     // office's hooks are in a Claude Code settings.json
    typing: bool,        // keys go to the panel, not the tree
    input: LineEdit,
    commands: Vec<Command>,                  // the panel session's slash commands
    commands_for: Option<(PathBuf, String)>, // its config dir and cwd: commands is theirs
    menu_at: usize,
    log_scroll: usize,     // how many lines the conversation is scrolled up from its newest
    log_max: Cell<usize>,  // how far it can be, as last drawn
    log_for: String,       // the session the scroll is for: another starts at the newest                           // the command picked in the menu of those matching the input
    note: String,     // what the last prompt or answer did
    ask: Option<Ask>, // the session's permission request waiting for an answer
    queued: Vec<String>, // as typed, oldest first: the prompts left for the session, not taken yet
    picks: Vec<String>, // answers given so far to the questions of the ask with id picks_for
    picks_for: String,
    live: bool, // a running Claude Code process holds the session
    resuming: Option<Resuming>,
    recall: Option<Recall>, // a walk through the history with ↑/↓, if one is on
    selection: Option<select::Selection>, // text dragged over in the tree view
    panes: Cell<[Rect; 3]>, // the insides of the tree, prompt and detail panes, as last drawn
    pub copied: Option<String>, // selected text for main to put on the clipboard
    here_for: Option<Here>, // the session whose background resume needs a terminal: o runs it here
    picker: Option<new::Picker>, // the new-session picker over the tree, while open
    select_started: Option<String>, // a session office started: selected once a scan shows it
}

/// A walk through the session's project's prompts: the prompt at shows,
/// draft is what was being typed when it began.
struct Recall {
    list: Vec<String>,
    at: usize,
    draft: String,
}

/// A `claude --bg` until it returns, and what explains its result.
struct Resuming {
    launch: Launch,
    child: std::process::Child,
    cwd: String,
    config_dir: PathBuf,
}

/// The fewest rows of the prompt panel, borders included; it takes two thirds
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
            hooks: false,
            typing: false,
            input: LineEdit::default(),
            commands: Vec::new(),
            commands_for: None,
            menu_at: 0,
            log_scroll: 0,
            log_max: Cell::new(0),
            log_for: String::new(),
            note: String::new(),
            ask: None,
            queued: Vec::new(),
            picks: Vec::new(),
            picks_for: String::new(),
            live: false,
            resuming: None,
            recall: None,
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
        if self.typing && self.view == View::Tree {
            self.scroll_log(if up { 3 } else { -3 });
            return Action::None;
        }
        self.key(KeyEvent::from(if up { KeyCode::Up } else { KeyCode::Down }))
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        let started = self.select_started.as_ref().and_then(|short| snap.roots.iter().find(|id| id.starts_with(short.as_str())).cloned());
        self.snap = Some(snap);
        if let Some(id) = started {
            (self.selected, self.select_started, self.typing) = (id, None, true);
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
        if let Some(p) = self.picker.as_mut() {
            p.paste(&text.replace(['\r', '\n'], ""));
        } else if self.typing {
            self.input.insert(text);
            (self.menu_at, self.recall) = (0, None);
        }
    }

    /// Handles a key press.
    pub fn key(&mut self, k: KeyEvent) -> Action {
        self.selection = None;
        let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
        if self.picker.is_some() && self.view == View::Tree && !ctrl_c {
            return self.picker_key(k);
        }
        if self.typing && self.view == View::Tree && !ctrl_c {
            return self.panel_key(k);
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
            // The tree, on the selected agent, typing in its session's panel.
            KeyCode::Char('p') => {
                let action = self.office_key(KeyCode::Enter);
                self.typing = true;
                return action;
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
            KeyCode::Char('p') => self.typing = true,
            KeyCode::Char('o') => return self.resume(),
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
        app.hooks = true;
        app.set_snapshot(snap.clone());
        app.selected = "a1".into();
        let text = |app: &App| app.panel_lines(40, 12).iter().map(Line::to_string).collect::<Vec<_>>();
        let t = text(&app);
        assert_eq!(t.len(), 12, "{t:#?}");
        assert!(t[1].starts_with("↑ ") && t[1].ends_with("more · pgup"), "older lines are above: {t:#?}");
        let end = t.iter().position(|l| l.starts_with("  Should I")).expect("the reply's end shows, over the box");
        assert_eq!((t[end].as_str(), t[end + 1].as_str()), ("  Should I also delete the old build", "  directory before rebuilding?"));
        assert!(t[end + 2].starts_with('╭'), "{t:#?}");

        // Page up shows older lines, page down the newest again.
        (app.view, app.typing) = (View::Tree, true);
        app.key(KeyEvent::from(KeyCode::PageUp));
        let t = text(&app);
        assert!(!t.iter().any(|l| l.starts_with("  Should I")) && t.iter().any(|l| l.starts_with("↓ ") && l.ends_with("newer · pgdn")), "{t:#?}");
        app.key(KeyEvent::from(KeyCode::PageDown));
        assert!(text(&app).iter().any(|l| l.starts_with("  Should I")));

        snap.by_id.get_mut("s1").unwrap().asking = "Which approach?\n  1. Fast — skip the cache\n  2. Safe".into();
        app.set_snapshot(snap);
        let t = text(&app);
        let at = t.iter().position(|l| l == "asks you:").expect("the question shows");
        assert_eq!(t[at + 1..at + 4], ["Which approach?", "  1. Fast — skip the cache", "  2. Safe"], "{t:#?}");
        assert!(t[at + 4].starts_with('╭'), "over the input box: {t:#?}");
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
        app.hooks = true;
        app.set_snapshot(snap);
        app.selected = "s1".into();
        let t = app.panel_lines(40, 14).iter().map(Line::to_string).collect::<Vec<_>>();
        let at = t.iter().position(|l| l == "> fix the build").expect("the prompt");
        assert_eq!(t[at + 1..at + 5], ["● Bash: cargo build", "", "● Fixed: a missing import in main.rs,", "  and the tests pass again."], "{t:#?}");
    }

    /// A background resume refused for trust offers to run it here: the keys
    /// go back to the tree, and o hands the terminal to that session.
    #[test]
    fn an_untrusted_resume_offers_to_run_it_here() {
        let mut app = App::new(0, 1);
        app.view = View::Tree;
        app.set_snapshot(busy_snap());
        app.selected = "s1".into();
        app.typing = true;
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
        assert!(app.note.contains("/tmp is not trusted") && app.note.contains("Press o to run it here"), "{}", app.note);
        assert!(!app.typing, "o is a key of the tree again");
        let want = Here { launch: Launch::Resume("s1".into()), cwd: "/tmp".into(), config_dir: "/cfg".into() };
        assert_eq!(press(&mut app, KeyCode::Char('o')), Action::RunHere(want.clone()));
        app.ran_here(&want, Ok(std::process::Command::new("true").status().unwrap()));
        assert!(app.note.starts_with("back from s1"), "{}", app.note);
        assert!(app.here_for.is_none(), "offered once");
    }

    /// A prompt sent to a session no process holds resumes it in the
    /// background, whose waiter then takes the prompt: no terminal needed.
    #[test]
    fn a_prompt_to_a_session_not_running_resumes_it() {
        let root = std::env::temp_dir().join(format!("office-autoresume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (fake, said) = (root.join("claude"), root.join("args"));
        std::fs::write(&fake, format!("#!/bin/sh\necho \"$@\" > {}\necho 'Started in the background'\n", said.display())).unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut app = App::new(0, 1);
        (app.bridge, app.hooks, app.claude) = (root.join("bridge"), true, fake);
        app.set_snapshot(busy_snap());
        app.focus = "s1".into();
        press(&mut app, KeyCode::Char('p'));
        for c in "check connection".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.note.starts_with("queued") && app.note.contains("resuming it in the background"), "{}", app.note);
        while app.resuming.is_some() {
            app.poll_resume();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_to_string(&said).unwrap().trim(), "--bg --resume s1");
        assert_eq!(app.note, "resumed in the background: it takes the waiting prompt now");
        assert_eq!(bridge::queued(&root.join("bridge"), "s1").len(), 1, "the prompt waits for the resumed session's waiter");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// n opens the picker over the tree: enter takes the folder a session
    /// ran in, the first prompt starts `claude --bg -- <prompt>` there, and
    /// the new session is selected, its panel typing, once a scan shows it.
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
        (app.bridge, app.hooks, app.claude, app.view) = (root.join("bridge"), true, fake, View::Tree);
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
        assert_eq!((app.selected.as_str(), app.typing), ("abcd1234-0000@work", true));
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
        assert_eq!(press(&mut app, KeyCode::Char('o')), Action::RunHere(want));
    }

    /// A long note wraps over several rows instead of being cut to one.
    #[test]
    fn panel_note_wraps_whole() {
        let mut app = App::new(0, 1);
        app.hooks = true;
        app.set_snapshot(busy_snap());
        app.selected = "s1".into();
        app.note = "not resumed: ~/dev/api is not trusted for this account yet. Run this once in a terminal and accept the trust prompt, then press o again: cd ~/dev/api && claude".into();
        let t = app.panel_lines(40, 20).iter().map(Line::to_string).collect::<Vec<_>>().join(" ");
        let note = t.split('╯').nth(1).expect("the note under the input box");
        assert!(note.contains("cd ~/dev/api && claude") && !note.contains('…'), "{t}");
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

    /// A waiting AskUserQuestion shows its question and numbered options; a
    /// digit picks an option, typed text answers in one's own words, and
    /// the last answer goes to the hook as question → answer.
    #[test]
    fn panel_answers_a_question_by_picking() {
        let root = std::env::temp_dir().join(format!("office-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("s1")).unwrap();
        let me = std::process::id(); // a live hook's pid: a dead one's question is not asked
        std::fs::write(root.join("s1").join(format!("ask-{me}.json")), r#"{"summary":"AskUserQuestion: Color?","questions":[
            {"question":"Color?","options":[{"label":"Red","description":"warm"},{"label":"Blue"}]},
            {"question":"Size?","options":[{"label":"S"},{"label":"L"}]}]}"#).unwrap();
        let mut app = App::new(0, 1);
        (app.bridge, app.hooks) = (root.clone(), true);
        app.set_snapshot(busy_snap());
        app.focus = "s1".into();
        press(&mut app, KeyCode::Char('p'));
        app.poll_bridge();
        let text = app.panel_lines(50, 20).iter().map(Line::to_string).collect::<Vec<_>>();
        let at = text.iter().position(|l| l == "asks you (1 of 2):").unwrap_or_else(|| panic!("{text:#?}"));
        assert_eq!(text[at + 1..at + 4], ["Color?", "  1. Red — warm", "  2. Blue"]);
        assert!(!text.iter().any(|l| l.contains("1 yes")), "no yes/no for a question: {text:#?}");
        press(&mut app, KeyCode::Char('2'));
        let text = app.panel_lines(50, 20).iter().map(Line::to_string).collect::<Vec<_>>();
        assert!(text.contains(&"asks you (2 of 2):".to_string()) && text.contains(&"Size?".to_string()), "{text:#?}");
        for c in "medium".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        let got: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("s1").join(format!("answer-{me}.json"))).unwrap()).unwrap();
        assert_eq!(got, serde_json::json!({ "answers": { "Color?": "Blue", "Size?": "medium" } }));
        assert!(bridge::queued(&root, "s1").is_empty(), "an answer, not a prompt");
        assert_eq!(app.note, "answered: Blue · medium");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A slash shows the session's commands: ↓ picks, tab completes, enter
    /// sends the picked one as its file to follow, a built-in stays typed,
    /// and ←/→ move the cursor to edit inside the prompt.
    #[test]
    fn slash_opens_a_command_menu_and_the_cursor_edits_inside_the_prompt() {
        let root = std::env::temp_dir().join(format!("office-menu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = App::new(0, 1);
        (app.bridge, app.hooks) = (root.clone(), true);
        app.set_snapshot(busy_snap());
        app.focus = "a1".into();
        press(&mut app, KeyCode::Char('p'));
        let s = app.panel_session().cloned().unwrap();
        let cmd = |n: &str, file: Option<&str>| Command { name: n.into(), about: format!("{n} it"), file: file.map(PathBuf::from) };
        app.commands = vec![cmd("compact", None), cmd("pstack:poteto-mode", Some("/p/poteto-mode/SKILL.md")), cmd("pstack:polish", Some("/p/polish/SKILL.md"))];
        app.commands_for = Some((s.config_dir.clone(), s.cwd.clone()));
        app.poll_bridge();
        for c in "/po".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let text = app.panel_lines(60, 30).iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
        assert!(text.contains("› /pstack:poteto-mode  pstack:poteto-mode it") && text.contains("  /pstack:polish"), "{text}");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), "/pstack:polish ");
        for c in "the bug".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        for _ in 0..7 {
            press(&mut app, KeyCode::Left);
        }
        for c in "fix ".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.input.text(), "/pstack:polish fix the bug");
        press(&mut app, KeyCode::Enter);
        let sent = bridge::cancel(&root, "s1").unwrap().expect("sent");
        assert!(sent.contains("Read /p/polish/SKILL.md in full") && sent.contains("arguments \"fix the bug\""), "{sent}");

        for c in "/comp".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(bridge::queued(&root, "s1").is_empty() && app.note.contains("built into Claude Code"), "{}", app.note);
        assert_eq!(app.input.text(), "/comp", "a built-in stays typed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ↑/↓ in the panel walk the project's prompts, Claude Code's own (pasted
    /// text filled in) and those sent from office, newest first; past the
    /// newest, what was being typed comes back.
    #[test]
    fn up_and_down_in_the_panel_recall_the_projects_prompts() {
        let root = std::env::temp_dir().join(format!("office-recall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (config, bridge) = (root.join("config"), root.join("bridge"));
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("history.jsonl"), concat!(
            r#"{"display":"first","pastedContents":{},"timestamp":1,"project":"/proj"}"#, "\n",
            r#"{"display":"elsewhere","pastedContents":{},"timestamp":2,"project":"/other"}"#, "\n",
            r#"{"display":"look at [Pasted text #1 +1 lines]","pastedContents":{"1":{"id":1,"type":"text","content":"a\nb"}},"timestamp":3,"project":"/proj"}"#, "\n",
        )).unwrap();
        let mut app = App::new(0, 1);
        (app.bridge, app.hooks) = (bridge.clone(), true);
        let mut snap = busy_snap();
        let s1 = snap.by_id.get_mut("s1").unwrap();
        (s1.config_dir, s1.cwd) = (config.clone(), "/proj".into());
        app.set_snapshot(snap);
        app.focus = "s1".into();
        press(&mut app, KeyCode::Char('p'));
        for c in "sent here".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        for c in "draft".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let mut seen = Vec::new();
        for _ in 0..4 {
            press(&mut app, KeyCode::Up);
            seen.push(app.input.text());
        }
        assert_eq!(seen, ["sent here", "look at a\nb", "first", "first"]);
        assert_eq!(app.selected, "s1", "the arrows recall, they do not move the tree");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('!'));
        assert_eq!(app.input.text(), "look at a\nb!", "an edit ends the walk, keeping the text");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.input.text(), "look at a\nb!", "no walk, no step");
        app.input.set("draft");
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.input.text(), "draft", "past the newest, the draft comes back");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A prompt sent while the session works shows in its conversation at
    /// once, as typed and marked not taken yet, and goes once the hook takes it.
    #[test]
    fn a_sent_prompt_shows_in_the_conversation_until_taken() {
        let root = std::env::temp_dir().join(format!("office-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = App::new(0, 1);
        (app.bridge, app.hooks) = (root.clone(), true);
        app.set_snapshot(busy_snap());
        app.focus = "s1".into();
        press(&mut app, KeyCode::Char('p'));
        for c in "fix the build".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        app.poll_bridge();
        let s = screen(&mut app);
        assert!(s.contains("> fix the build"), "{s}");
        assert!(s.contains("not taken yet"), "{s}");

        for c in "then test".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        app.poll_bridge();
        let s = screen(&mut app);
        let (first, second) = (s.find("> fix the build"), s.find("> then test"));
        assert!(first.is_some() && second.is_some() && first < second, "both wait, in the order sent: {s}");

        for _ in 0..2 {
            bridge::cancel(&root, "s1").unwrap().expect("waiting"); // as the hook takes it
        }
        app.poll_bridge();
        let s = screen(&mut app);
        assert!(!s.contains("fix the build") && !s.contains("not taken yet"), "{s}");
        let _ = std::fs::remove_dir_all(&root);
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

    /// ctrl+x takes back a prompt the session has not taken yet, into the
    /// box to edit; once taken, there is nothing to take back.
    #[test]
    fn ctrl_x_takes_back_a_prompt_not_taken_yet() {
        let root = std::env::temp_dir().join(format!("office-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = App::new(0, 1);
        (app.bridge, app.hooks) = (root.clone(), true);
        app.set_snapshot(busy_snap());
        app.focus = "s1".into();
        press(&mut app, KeyCode::Char('p'));
        for c in "fix the build".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        let ctrl_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(app.key(ctrl_x), Action::None);
        app.poll_bridge();
        assert!(bridge::queued(&root, "s1").is_empty());
        assert_eq!(app.input.text(), "fix the build");
        assert!(app.note.starts_with("taken back"), "{}", app.note);
        assert!(!screen(&mut app).contains("not taken yet"));

        app.input.clear();
        app.key(ctrl_x);
        assert!(app.note.starts_with("nothing to take back"), "{}", app.note);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A paste with line breaks is one prompt, sent on enter only: its
    /// breaks show as ↵ and reach the session as they were. Into the
    /// picker it is a path, its breaks gone; with the tree's keys it does nothing.
    #[test]
    fn a_paste_is_one_prompt_however_many_lines() {
        let root = std::env::temp_dir().join(format!("office-paste-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = App::new(0, 1);
        (app.bridge, app.hooks, app.view) = (root.clone(), true, View::Tree);
        app.set_snapshot(busy_snap());
        app.selected = "s1".into();
        app.paste("q");
        assert!(app.input.is_empty(), "the tree's keys take no paste");
        press(&mut app, KeyCode::Char('p'));
        app.paste("got error, not started:\r\nback when you exit");
        assert!(bridge::queued(&root, "s1").is_empty(), "nothing sent before enter");
        let text = app.panel_lines(80, 20).iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
        assert!(text.contains("got error, not started:↵back when you exit"), "{text}");
        press(&mut app, KeyCode::Enter);
        let sent = bridge::queued(&root, "s1");
        assert!(sent.len() == 1 && sent[0].ends_with("got error, not started:\nback when you exit"), "{sent:?}");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('n'));
        app.paste("/tmp\n");
        assert_eq!(app.picker.as_ref().map(|p| p.input_text()), Some("/tmp".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// p on an agent opens the tree on it with the panel taking keys: text
    /// (q too) builds the prompt, enter leaves it for its session, a digit
    /// on an empty prompt answers the session's permission request, esc
    /// gives the keys back to the tree.
    #[test]
    fn p_opens_the_prompt_panel_for_the_agents_session() {
        let root = std::env::temp_dir().join(format!("office-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = App::new(0, 1);
        (app.bridge, app.hooks) = (root.clone(), true);
        app.set_snapshot(busy_snap());
        app.focus = "a1".into();
        assert_eq!(press(&mut app, KeyCode::Char('p')), Action::Switched);
        assert_eq!((app.view, app.typing, app.selected.as_str()), (View::Tree, true, "a1"));
        assert_eq!(app.panel_session().map(|a| a.id.as_str()), Some("s1"), "a subagent's panel talks to its session");
        for c in "quit now".chars() {
            assert_eq!(press(&mut app, KeyCode::Char(c)), Action::None);
        }
        press(&mut app, KeyCode::Enter);
        assert!(bridge::queued(&root, "s1")[0].ends_with("next message:\n\nquit now"));
        assert!(app.input.is_empty() && app.note.starts_with("queued"), "{}", app.note);

        // A permission request filed by the hook; 2 is "yes, don't ask again".
        let me = std::process::id(); // a live hook's pid: a dead one's request is not asked
        let filed = serde_json::json!({
            "summary": "Bash: rm -rf build", "tool": "Bash", "input": { "command": "rm -rf build &&\n  make" },
            "suggestions": [{ "type": "addRules", "rules": [{ "toolName": "Bash", "ruleContent": "rm -rf build:*" }], "behavior": "allow", "destination": "localSettings" }],
        });
        std::fs::write(root.join("s1").join(format!("ask-{me}.json")), filed.to_string()).unwrap();
        app.poll_bridge();
        assert_eq!(app.ask.as_ref().map(|a| a.summary.as_str()), Some("Bash: rm -rf build"));
        let text = app.panel_lines(60, 30).iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
        assert!(text.contains("asks: Bash: rm -rf build") && text.contains("1 yes · 2 yes, don't ask again · 3 no"), "{text}");
        assert!(text.contains("  rm -rf build &&\n    make"), "the whole call: {text}");
        assert!(text.contains("2 saves: Bash(rm -rf build:*) (this project, just you)"), "{text}");
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(std::fs::read_to_string(root.join("s1").join(format!("answer-{me}.json"))).unwrap(), "always");
        assert_eq!(press(&mut app, KeyCode::Esc), Action::None);
        assert!(!app.typing);
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
