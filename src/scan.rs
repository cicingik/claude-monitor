//! Reads Claude Code transcripts under one or more projects directories and
//! builds snapshots of sessions, subagents, their interactions and token
//! usage. It only reads.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, DirEntry, Metadata},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

mod text;
mod transcript;

pub(crate) use text::tool_summary;
pub use text::{days_from_civil, parse_rfc3339, truncate};
use transcript::{FileState, Meta, build, project_name, read_meta};

/// Nanoseconds in a second. Times are i64 Unix nanoseconds; 0 is "never",
/// like Go's zero time.
pub const SEC: i64 = 1_000_000_000;
/// How many recent events are kept per agent.
pub const MAX_EVENTS: usize = 50;
const RUNNING_WINDOW: i64 = 120 * SEC;
const ACTIVE_WINDOW: i64 = 30 * SEC;
/// A tool call that may need the user is taken as waiting once nothing has
/// been written for WAIT_QUIET (a call that needs no one answers sooner),
/// and for at most WAIT_WINDOW (past that the session was likely closed).
const WAIT_QUIET: i64 = 3 * SEC;
const WAIT_WINDOW: i64 = 3600 * SEC;
/// A session quiet this long counts as inactive, though a Claude Code
/// process still holds it: a terminal left open for days.
pub const IDLE_LIMIT: i64 = 24 * 3600 * SEC;
/// The longest reply or question kept whole.
const MAX_TEXT: usize = 4000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Session,
    Subagent,
}

/// The liveness of an agent, from its transcript and file mtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Status {
    #[default]
    Idle,    // nothing written for 30s or more
    Active,  // transcript written less than 30s ago
    Running, // last tool_use has no tool_result yet and file written less than 2m ago
    Waiting, // last tool_use waits for the user: a question, a plan, or a permission prompt
    Done,    // subagent meta says finished
}

impl Status {
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Running => "●",
            Status::Waiting => "?",
            Status::Active => "◐",
            Status::Done => "✓",
            Status::Idle => "○",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Waiting => "waiting",
            Status::Active => "active",
            Status::Done => "done",
            Status::Idle => "idle",
        }
    }
}

/// A token count, deduplicated per API message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

impl Usage {
    pub fn total(self) -> i64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    pub fn add(self, o: Usage) -> Usage {
        Usage {
            input: self.input + o.input,
            output: self.output + o.output,
            cache_read: self.cache_read + o.cache_read,
            cache_write: self.cache_write + o.cache_write,
        }
    }
}

/// One entry of an agent's recent activity. The summary is always short;
/// a prompt or a reply also keeps its whole text, for the prompt panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Event {
    pub at: i64,
    pub kind: &'static str, // tool_use | text | prompt | handback
    pub tool: String,
    pub summary: String,
    pub text: String, // a prompt's or reply's whole text, at most MAX_TEXT chars; empty for the others
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    Spawn,
    Message,
    Handback,
}

impl EdgeKind {
    pub fn name(self) -> &'static str {
        match self {
            EdgeKind::Spawn => "spawn",
            EdgeKind::Message => "message",
            EdgeKind::Handback => "handback",
        }
    }
}

/// An interaction between two agents.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub at: i64,
    pub label: String,
}

/// A session or subagent as of one scan.
#[derive(Clone, Debug, Default)]
pub struct Agent {
    pub id: String,
    pub parent_id: String,
    pub title: String,
    pub agent_type: String,
    pub model: String,
    pub project: String,
    pub session_id: String, // the root session (equals id for sessions)
    pub config: String,     // which config dir: "claude", "personal", "work", ...
    pub config_dir: PathBuf, // that config dir itself (~/.claude-work), to resume the session in
    pub cwd: String,         // the session's working directory
    pub kind: Kind,
    pub status: Status,
    pub last_activity: i64,
    pub last_tool: String, // summary of the most recent tool_use
    pub last_text: String, // its most recent reply text, whole (at most MAX_TEXT chars)
    pub asking: String,    // what its unanswered call asks the user: questions and options, or a plan
    pub turns: usize,      // distinct message.id
    pub usage: Usage,      // this agent's own usage, not its children's
    pub model_usage: BTreeMap<String, Usage>,
    pub events: Vec<Event>,    // newest last, at most MAX_EVENTS
    pub children: Vec<String>, // ids, oldest first
    pub live: bool,            // a session a running Claude Code process holds, quiet for at most IDLE_LIMIT
    pub continued: bool,       // its conversation moved on to another session id: nothing listens here
}

/// The view built by one scan.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub roots: Vec<String>, // session ids, most recently active first
    pub by_id: HashMap<String, Agent>,
    pub edges: Vec<Edge>, // oldest first
    pub by_model: BTreeMap<String, Usage>,
    pub scanned_at: i64,
}

impl Snapshot {
    /// Visits id and its descendants depth-first, passing the depth.
    pub fn walk<'a>(&'a self, id: &str, depth: usize, f: &mut impl FnMut(&'a Agent, usize)) {
        if let Some(a) = self.by_id.get(id) {
            f(a, depth);
            for c in &a.children {
                self.walk(c, depth + 1, f);
            }
        }
    }

    /// The usage of id and all its descendants.
    pub fn subtree_usage(&self, id: &str) -> Usage {
        let mut u = Usage::default();
        self.walk(id, 0, &mut |a, _| u = u.add(a.usage));
        u
    }
}

/// A projects directory and the short name of its config dir.
#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    pub dir: PathBuf,
    pub label: String,
}

impl Root {
    /// `~/.claude-work/projects` is "work", `~/.claude/projects` is "claude".
    pub fn new(dir: PathBuf) -> Root {
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
        let base = match name(&dir) {
            Some(n) if n == "projects" => dir.parent().and_then(name).unwrap_or(n),
            Some(n) => n,
            None => dir.display().to_string(),
        };
        let base = base.trim_start_matches('.');
        let label = base.strip_prefix("claude-").filter(|l| !l.is_empty()).unwrap_or(base);
        Root { label: if label.is_empty() { dir.display().to_string() } else { label.to_string() }, dir }
    }
}

/// `~/.claude/projects` and every `~/.claude-*/projects` that exists.
pub fn default_roots(home: &Path) -> Vec<Root> {
    let mut dirs = vec![home.join(".claude").join("projects")];
    let mut extra: Vec<PathBuf> = fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".claude-"))
        .map(|e| e.path().join("projects"))
        .filter(|p| p.is_dir())
        .collect();
    extra.sort();
    dirs.extend(extra);
    dedup_roots(dirs)
}

/// Roots for these dirs, dropping any that is the same directory as an
/// earlier one (through a symlink, say), so no transcript is read twice.
pub fn dedup_roots(dirs: Vec<PathBuf>) -> Vec<Root> {
    let mut seen = HashSet::new();
    dirs.into_iter().filter(|d| seen.insert(fs::canonicalize(d).unwrap_or_else(|_| d.clone()))).map(Root::new).collect()
}

/// Keeps per-file parse state between scans, so each scan reads only the
/// lines appended since the last one.
pub struct Scanner {
    pub roots: Vec<Root>,
    pub since: i64, // nanoseconds; sessions whose file is older are skipped; <= 0 keeps all
    files: HashMap<PathBuf, FileState>,
}

fn sorted_dir(dir: &Path) -> Vec<DirEntry> {
    let mut v: Vec<DirEntry> = fs::read_dir(dir).into_iter().flatten().flatten().collect();
    v.sort_by_key(|e| e.file_name());
    v
}

/// Unix nanoseconds of a file's mtime.
fn mtime(md: &Metadata) -> i64 {
    md.modified().ok().map_or(0, unix_nanos)
}

pub fn unix_nanos(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    }
}

/// One member of a session being built: an agent and where it came from.
struct Member {
    id: String,
    path: PathBuf,
    mtime: i64,
    meta: Meta,
}

impl Scanner {
    pub fn new(roots: Vec<Root>, since: i64) -> Scanner {
        Scanner { roots, since, files: HashMap::new() }
    }

    /// Drops all parse state so the next scan re-reads every file.
    pub fn reset(&mut self) {
        self.files.clear();
    }

    /// Reads new transcript lines and returns a fresh snapshot.
    pub fn scan(&mut self, now: i64) -> Snapshot {
        let mut snap = Snapshot { scanned_at: now, ..Snapshot::default() };
        let mut seen = HashSet::new();
        let mut live: HashMap<PathBuf, HashSet<String>> = HashMap::new();
        for root in self.roots.clone() {
            let config_dir = root.dir.parent().map(Path::to_path_buf).unwrap_or_default();
            let held = live.entry(config_dir.clone()).or_insert_with(|| crate::bridge::live_ids(&config_dir)).clone();
            for p in sorted_dir(&root.dir) {
                if !p.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let proj_dir = p.path();
                let proj_name = p.file_name().to_string_lossy().into_owned();
                for e in sorted_dir(&proj_dir) {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if e.file_type().is_ok_and(|t| t.is_dir()) || !name.ends_with(".jsonl") {
                        continue;
                    }
                    let Ok(md) = e.metadata() else { continue };
                    let sid = name.trim_end_matches(".jsonl");
                    if self.since > 0 && now - mtime(&md) > self.since && !(held.contains(sid) && now - mtime(&md) <= IDLE_LIMIT) {
                        continue;
                    }
                    self.session(&mut snap, &mut seen, &root, &proj_dir, &proj_name, sid, &md, now);
                }
            }
        }
        self.files.retain(|p, _| seen.contains(p));
        for id in &snap.roots {
            let a = snap.by_id.get_mut(id).expect("a root is an agent");
            let held = live.get(&a.config_dir).is_some_and(|held| held.contains(id.split('@').next().unwrap_or(id)));
            a.live = !a.continued && held && now - a.last_activity <= IDLE_LIMIT;
        }
        let by_id = &snap.by_id;
        snap.roots.sort_by(|a, b| by_id[b].last_activity.cmp(&by_id[a].last_activity));
        snap.edges.sort_by_key(|e| e.at);
        for a in snap.by_id.values() {
            for (m, u) in &a.model_usage {
                let t = snap.by_model.entry(m.clone()).or_default();
                *t = t.add(*u);
            }
        }
        snap
    }

    /// Advances the parse state of path to the current end of file.
    fn load(&mut self, seen: &mut HashSet<PathBuf>, path: &Path, size: u64) {
        seen.insert(path.to_path_buf());
        let st = self.files.entry(path.to_path_buf()).or_default();
        if size < st.offset {
            *st = FileState::default(); // truncated and rewritten
        }
        if size > st.offset {
            let _ = st.read_from(path); // unreadable: keep what was parsed
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn session(&mut self, snap: &mut Snapshot, seen: &mut HashSet<PathBuf>, root: &Root, proj_dir: &Path, proj_name: &str, sid: &str, md: &Metadata, now: i64) {
        let path = proj_dir.join(format!("{sid}.jsonl"));
        self.load(seen, &path, md.len());
        let mut members = vec![Member { id: sid.to_string(), path, mtime: mtime(md), meta: Meta::default() }];
        let sub_dir = proj_dir.join(sid).join("subagents");
        for e in sorted_dir(&sub_dir) {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_dir()) || !name.starts_with("agent-") || !name.ends_with(".jsonl") {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            let id = name.trim_start_matches("agent-").trim_end_matches(".jsonl").to_string();
            let meta = read_meta(&sub_dir.join(format!("agent-{id}.meta.json")));
            let p = sub_dir.join(&name);
            self.load(seen, &p, md.len());
            members.push(Member { id, path: p, mtime: mtime(&md), meta });
        }

        // The same session can sit under two roots (a copied config dir): the
        // later one gets "@<root>" on every id, so ids stay unique.
        let suffix = if snap.by_id.contains_key(sid) { format!("@{}", root.label) } else { String::new() };
        let key = |id: &str| format!("{id}{suffix}");
        let files = &self.files;
        let st = |m: &Member| &files[&m.path];
        let index: HashMap<&str, usize> = members.iter().enumerate().map(|(i, m)| (m.id.as_str(), i)).collect();

        // Which transcript holds each Agent/Task tool_use, and spawn names.
        let mut owner: HashMap<&str, usize> = HashMap::new();
        for (i, m) in members.iter().enumerate() {
            for tid in st(m).spawns.keys() {
                owner.insert(tid, i);
            }
        }
        let mut parent: Vec<usize> = vec![0; members.len()];
        let mut by_name: HashMap<&str, usize> = HashMap::new();
        for i in 1..members.len() {
            let tid = members[i].meta.tool_use_id.as_str();
            if let Some(&o) = owner.get(tid).filter(|&&o| o != i) {
                parent[i] = o;
            }
            if let Some(sp) = st(&members[parent[i]]).spawns.get(tid).filter(|sp| !sp.name.is_empty()) {
                by_name.insert(sp.name.as_str(), i);
            }
        }
        for i in 1..members.len() {
            if cyclic(&parent, i) {
                parent[i] = 0; // break the cycle by reattaching to the session
            }
        }

        let root_st = st(&members[0]);
        let project = project_name(&root_st.cwd, proj_name);
        let mut agents: Vec<Agent> = members
            .iter()
            .enumerate()
            .map(|(i, m)| {
                // Subagents' files hold no mode: they run in their session's.
                let mut a = build(st(m), key(&m.id), m.mtime, now, m.meta.finished, &root_st.mode);
                a.session_id = key(sid);
                a.config = root.label.clone();
                a.config_dir = root.dir.parent().map(Path::to_path_buf).unwrap_or_default();
                a.cwd = root_st.cwd.clone();
                a.project = project.clone();
                if i == 0 {
                    a.title = truncate(&root_st.title, 60);
                } else {
                    a.kind = Kind::Subagent;
                    a.parent_id = key(&members[parent[i]].id);
                    a.title = truncate(&m.meta.description, 60);
                    a.agent_type = m.meta.agent_type.clone();
                }
                a
            })
            .collect();

        let mut children: Vec<Vec<usize>> = vec![Vec::new(); members.len()];
        for i in 1..members.len() {
            let m = &members[i];
            children[parent[i]].push(i);
            let sp = st(&members[parent[i]]).spawns.get(&m.meta.tool_use_id);
            let mut e = Edge {
                from: key(&members[parent[i]].id),
                to: key(&m.id),
                kind: EdgeKind::Spawn,
                at: sp.map_or(0, |s| s.at),
                label: truncate(sp.map_or("", |s| &s.description), 40),
            };
            if e.at == 0 {
                e.at = st(m).first_at;
            }
            if e.label.is_empty() {
                e.label = truncate(&m.meta.description, 40);
            }
            snap.edges.push(e);
        }
        for (i, m) in members.iter().enumerate() {
            children[i].sort_by_key(|&c| st(&members[c]).first_at);
            agents[i].children = children[i].iter().map(|&c| key(&members[c].id)).collect();
            for sd in &st(m).sends {
                let to = match index.get(sd.to.as_str()).or_else(|| by_name.get(sd.to.as_str())) {
                    Some(&j) => key(&members[j].id),
                    None => sd.to.clone(),
                };
                snap.edges.push(Edge { from: key(&m.id), to, kind: EdgeKind::Message, at: sd.at, label: sd.label.clone() });
            }
            for hb in &st(m).handbacks {
                let from = if index.contains_key(hb.to.as_str()) { key(&hb.to) } else { hb.to.clone() };
                snap.edges.push(Edge { from, to: key(&m.id), kind: EdgeKind::Handback, at: hb.at, label: "handback".into() });
            }
        }
        snap.roots.push(key(sid));
        for a in agents {
            snap.by_id.insert(a.id.clone(), a);
        }
    }
}

fn cyclic(parent: &[usize], i: usize) -> bool {
    let mut visited = HashSet::new();
    let mut cur = i;
    while cur != 0 {
        if !visited.insert(cur) {
            return true;
        }
        cur = parent[cur];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn root_labels_name_the_config_dir() {
        assert_eq!(Root::new("/h/.claude/projects".into()).label, "claude");
        assert_eq!(Root::new("/h/.claude-personal/projects".into()).label, "personal");
        assert_eq!(Root::new("/tmp/fixture".into()).label, "fixture");
    }

    pub(super) fn write(path: &Path, lines: &[&str]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    /// A session that spawns one subagent, which messages it and hands back,
    /// with usage repeated on two lines of one message.
    #[test]
    fn scan_builds_tree_usage_edges_and_reads_incrementally() {
        let dir = std::env::temp_dir().join(format!("office-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let proj = dir.join("projects").join("-home-me-proj");
        let sid = "s1";
        let main = proj.join(format!("{sid}.jsonl"));
        write(&main, &[
            r#"{"type":"user","timestamp":"2026-01-01T10:00:00Z","cwd":"/home/me/proj","message":{"content":"fix the bug please"}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T10:00:01Z","message":{"id":"m1","model":"claude-opus-4-5","usage":{"input_tokens":10,"output_tokens":5},"content":[{"type":"text","text":"ok"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T10:00:02Z","message":{"id":"m1","model":"claude-opus-4-5","usage":{"input_tokens":10,"output_tokens":5},"content":[{"type":"tool_use","id":"tu1","name":"Agent","input":{"description":"explore code","subagent_type":"Explore","name":"scout"}}]}}"#,
            "not json",
            r#"{"type":"ai-title","aiTitle":"Fix bug"}"#,
            r#"{"type":"user","timestamp":"2026-01-01T10:01:00Z","message":{"content":[{"type":"text","text":"<agent-message from=\"a1\">done</agent-message>"}]}}"#,
        ]);
        let sub = proj.join(sid).join("subagents");
        write(&sub.join("agent-a1.jsonl"), &[
            r#"{"type":"assistant","timestamp":"2026-01-01T10:00:03Z","message":{"id":"x1","model":"claude-sonnet-4-5-20250929","usage":{"input_tokens":100,"output_tokens":7,"cache_read_input_tokens":3},"content":[{"type":"tool_use","id":"tu2","name":"SendMessage","input":{"to":"s1","message":"found it\nmore"}}]}}"#,
        ]);
        fs::write(sub.join("agent-a1.meta.json"), r#"{"agentType":"Explore","description":"explore code","toolUseId":"tu1","finished":true}"#).unwrap();

        let mut s = Scanner::new(vec![Root::new(dir.join("projects"))], 0);
        let now = parse_rfc3339("2026-01-01T12:00:00Z").unwrap();
        let snap = s.scan(now);
        assert_eq!(snap.roots, ["s1"]);
        let root = &snap.by_id["s1"];
        assert_eq!((root.title.as_str(), root.project.as_str(), root.turns), ("Fix bug", "proj", 1));
        assert_eq!(root.usage, Usage { input: 10, output: 5, ..Usage::default() });
        assert_eq!(root.children, ["a1"]);
        let a = &snap.by_id["a1"];
        assert_eq!((a.parent_id.as_str(), a.status, a.agent_type.as_str()), ("s1", Status::Done, "Explore"));
        assert_eq!(a.model_usage["claude-sonnet-4-5-20250929"].total(), 110);
        let kinds: Vec<_> = snap.edges.iter().map(|e| (e.kind, e.from.as_str(), e.to.as_str(), e.label.as_str())).collect();
        assert_eq!(kinds, [
            (EdgeKind::Spawn, "s1", "a1", "explore code"),
            (EdgeKind::Message, "a1", "s1", "found it"),
            (EdgeKind::Handback, "a1", "s1", "handback"),
        ]);
        assert_eq!(snap.subtree_usage("s1").total(), 125);

        // Appended lines are read on the next scan; repeated usage still counts once.
        write(&main, &[r#"{"type":"assistant","timestamp":"2026-01-01T10:02:00Z","message":{"id":"m2","model":"claude-opus-4-5","usage":{"output_tokens":1},"content":[{"type":"tool_use","id":"tu3","name":"Bash","input":{"command":"ls -la\nmore"}}]}}"#]);
        let snap = s.scan(now);
        let root = &snap.by_id["s1"];
        assert_eq!((root.turns, root.usage.total(), root.last_tool.as_str()), (2, 16, "Bash: ls -la more"));

        // The same session under a second root keeps unique ids.
        let copy = dir.join("other").join(".claude-work").join("projects").join("-home-me-proj");
        write(&copy.join("s1.jsonl"), &[r#"{"type":"user","timestamp":"2026-01-01T09:00:00Z","message":{"content":"hi"}}"#]);
        let mut s = Scanner::new(vec![Root::new(dir.join("projects")), Root::new(copy.parent().unwrap().into())], 0);
        let snap = s.scan(now);
        assert_eq!(snap.roots, ["s1", "s1@work"]);
        assert_eq!(snap.by_id["s1@work"].config, "work");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A session a live process names in sessions/<pid>.json is live, even
    /// with its transcript older than `since`, as a background session
    /// resumed hours after its last turn, until it has been quiet for
    /// IDLE_LIMIT (a terminal left open for days); one whose process is
    /// gone is not.
    #[test]
    fn a_session_a_running_process_holds_is_live_until_idle_limit() {
        let dir = std::env::temp_dir().join(format!("office-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let cfg = dir.join(".claude-work");
        let line = r#"{"type":"user","cwd":"/p","message":{"content":"hi"}}"#; // no timestamp: last activity is the file's mtime
        for s in ["held", "gone"] {
            write(&cfg.join("projects").join("-p").join(format!("{s}.jsonl")), &[line]);
        }
        fs::create_dir_all(cfg.join("sessions")).unwrap();
        fs::write(cfg.join("sessions").join("1.json"), format!(r#"{{"pid":{},"sessionId":"held","kind":"bg"}}"#, std::process::id())).unwrap();
        fs::write(cfg.join("sessions").join("2.json"), r#"{"pid":999999999,"sessionId":"gone","kind":"bg"}"#).unwrap();
        let at = mtime(&fs::metadata(cfg.join("projects").join("-p").join("held.jsonl")).unwrap());
        let now = at + 2 * 3600 * SEC;
        let snap = Scanner::new(vec![Root::new(cfg.join("projects"))], 3600 * SEC).scan(now);
        assert_eq!(snap.roots, ["held"], "a quiet session no process holds is past `since`");
        assert!(snap.by_id["held"].live);
        let snap = Scanner::new(vec![Root::new(cfg.join("projects"))], 0).scan(now);
        assert!(snap.by_id["held"].live && !snap.by_id["gone"].live);

        // `/resume` in a terminal moves its conversation on to another id:
        // the process still names the old one, but nothing listens there.
        let moved = r#"{"type":"continued-in","timestamp":"2026-01-01T09:01:00Z","sessionId":"moved","continuedInSessionId":"other"}"#;
        write(&cfg.join("projects").join("-p").join("moved.jsonl"), &[line, moved]);
        fs::write(cfg.join("sessions").join("3.json"), format!(r#"{{"pid":{},"sessionId":"moved","kind":"interactive"}}"#, std::process::id())).unwrap();
        let snap = Scanner::new(vec![Root::new(cfg.join("projects"))], 0).scan(now);
        assert!(snap.by_id["held"].live && !snap.by_id["moved"].live);

        // Held, but quiet past IDLE_LIMIT: inactive, like any other session.
        let past = at + IDLE_LIMIT + SEC;
        assert!(!Scanner::new(vec![Root::new(cfg.join("projects"))], 0).scan(past).by_id["held"].live);
        assert!(!Scanner::new(vec![Root::new(cfg.join("projects"))], 3600 * SEC).scan(past).roots.contains(&"held".to_string()), "and past `since`");
        let _ = fs::remove_dir_all(&dir);
    }
}
