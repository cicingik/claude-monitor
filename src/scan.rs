//! Reads Claude Code transcripts under one or more projects directories and
//! builds snapshots of sessions, subagents, their interactions and token
//! usage. It only reads.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, DirEntry, File, Metadata},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

/// Nanoseconds in a second. Times are i64 Unix nanoseconds; 0 is "never",
/// like Go's zero time.
pub const SEC: i64 = 1_000_000_000;
/// How many recent events are kept per agent.
pub const MAX_EVENTS: usize = 50;
const RUNNING_WINDOW: i64 = 120 * SEC;
const ACTIVE_WINDOW: i64 = 30 * SEC;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Session,
    Subagent,
}

/// The liveness of an agent, from its transcript and file mtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Idle,    // nothing written for 30s or more
    Active,  // transcript written less than 30s ago
    Running, // last tool_use has no tool_result yet and file written less than 2m ago
    Done,    // subagent meta says finished
}

impl Status {
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Running => "●",
            Status::Active => "◐",
            Status::Done => "✓",
            Status::Idle => "○",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Status::Running => "running",
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

/// One entry of an agent's recent activity. The summary is always short.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub at: i64,
    pub kind: &'static str, // tool_use | text | prompt | handback
    pub tool: String,
    pub summary: String,
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
#[derive(Clone, Debug)]
pub struct Agent {
    pub id: String,
    pub parent_id: String,
    pub title: String,
    pub agent_type: String,
    pub model: String,
    pub project: String,
    pub session_id: String, // the root session (equals id for sessions)
    pub config: String,     // which config dir: "claude", "personal", "work", ...
    pub kind: Kind,
    pub status: Status,
    pub last_activity: i64,
    pub last_tool: String, // summary of the most recent tool_use
    pub turns: usize,      // distinct message.id
    pub usage: Usage,      // this agent's own usage, not its children's
    pub model_usage: BTreeMap<String, Usage>,
    pub events: Vec<Event>,    // newest last, at most MAX_EVENTS
    pub children: Vec<String>, // ids, oldest first
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

/// The subset of agent-<id>.meta.json the monitor uses.
#[derive(Clone, Debug, Default)]
struct Meta {
    agent_type: String,
    description: String,
    tool_use_id: String,
    finished: bool,
}

/// An Agent/Task tool_use seen in a transcript.
#[derive(Clone, Debug, Default)]
struct Spawn {
    at: i64,
    description: String,
    name: String, // optional input.name, resolves SendMessage targets
}

/// A SendMessage tool_use, or a handback notice (`to` is then the sender).
#[derive(Clone, Debug)]
struct Sent {
    at: i64,
    to: String,
    label: String,
}

/// The parsed state of one transcript, advanced incrementally.
#[derive(Debug, Default)]
struct FileState {
    offset: u64, // bytes consumed: always ends just after a '\n'
    title: String,
    model: String,
    cwd: String,
    first_at: i64,
    last_at: i64,
    usage: HashMap<String, Usage>,      // per message.id, so repeated lines count once
    msg_model: HashMap<String, String>, // message.id -> model
    events: Vec<Event>,
    last_tool_id: String,
    last_tool: String,
    resolved: HashSet<String>,     // tool_use ids that have a tool_result
    spawns: HashMap<String, Spawn>, // tool_use id -> Agent/Task call
    sends: Vec<Sent>,
    handbacks: Vec<Sent>,
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
        for root in self.roots.clone() {
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
                    if self.since > 0 && now - mtime(&md) > self.since {
                        continue;
                    }
                    let sid = name.trim_end_matches(".jsonl");
                    self.session(&mut snap, &mut seen, &root, &proj_dir, &proj_name, sid, &md, now);
                }
            }
        }
        self.files.retain(|p, _| seen.contains(p));
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
                let mut a = build(st(m), key(&m.id), m.mtime, now, m.meta.finished);
                a.session_id = key(sid);
                a.config = root.label.clone();
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

fn read_meta(path: &Path) -> Meta {
    let Ok(b) = fs::read(path) else { return Meta::default() };
    let Ok(v) = serde_json::from_slice::<Value>(&b) else { return Meta::default() };
    Meta {
        agent_type: s(&v, "agentType").into(),
        description: s(&v, "description").into(),
        tool_use_id: s(&v, "toolUseId").into(),
        finished: v.get("finished").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Turns parse state into an agent with status, usage and events.
fn build(st: &FileState, id: String, mtime: i64, now: i64, finished: bool) -> Agent {
    let mut a = Agent {
        id,
        parent_id: String::new(),
        title: String::new(),
        agent_type: String::new(),
        model: st.model.clone(),
        project: String::new(),
        session_id: String::new(),
        config: String::new(),
        kind: Kind::Session,
        status: Status::Idle,
        last_activity: if st.last_at == 0 { mtime } else { st.last_at },
        last_tool: st.last_tool.clone(),
        turns: st.usage.len(),
        usage: Usage::default(),
        model_usage: BTreeMap::new(),
        events: st.events.clone(),
        children: Vec::new(),
    };
    for (mid, u) in &st.usage {
        a.usage = a.usage.add(*u);
        if *u == Usage::default() {
            continue; // e.g. <synthetic> messages: no tokens, no model bucket
        }
        let m = st.msg_model.get(mid).filter(|m| !m.is_empty()).map_or("unknown", |m| m.as_str());
        let t = a.model_usage.entry(m.to_string()).or_default();
        *t = t.add(*u);
    }
    let age = now - mtime;
    a.status = if finished {
        Status::Done
    } else if st.pending() && age < RUNNING_WINDOW {
        Status::Running
    } else if age < ACTIVE_WINDOW {
        Status::Active
    } else {
        Status::Idle
    };
    a
}

/// The last segment of the session cwd, or of the encoded dir name.
fn project_name(cwd: &str, dir: &str) -> String {
    if !cwd.is_empty() {
        let t = cwd.trim_end_matches('/');
        return if t.is_empty() { "/".into() } else { t.rsplit('/').next().unwrap_or(t).into() };
    }
    match dir.rfind('-') {
        Some(i) if i < dir.len() - 1 => dir[i + 1..].into(),
        _ => dir.into(),
    }
}

/// The string at key in v, or "" when it is missing or not a string.
fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

impl FileState {
    /// Whether the most recent tool_use still waits for its result.
    fn pending(&self) -> bool {
        !self.last_tool_id.is_empty() && !self.resolved.contains(&self.last_tool_id)
    }

    /// Consumes complete lines from the offset to EOF. A trailing line
    /// without '\n' is left for the next call.
    fn read_from(&mut self, path: &Path) -> std::io::Result<()> {
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut r = BufReader::with_capacity(256 * 1024, f);
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = r.read_until(b'\n', &mut line)?;
            if n == 0 || line.last() != Some(&b'\n') {
                return Ok(()); // partial or empty tail: not consumed
            }
            self.offset += n as u64;
            self.parse_line(&line);
        }
    }

    fn parse_line(&mut self, b: &[u8]) {
        if !may_be_relevant(b) {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(b) else { return }; // malformed: skip
        // Go decodes timestamp as time.Time: a bad one fails the whole line.
        let at = match v.get("timestamp") {
            None | Some(Value::Null) => 0,
            Some(Value::String(t)) => match parse_rfc3339(t) {
                Some(t) => t,
                None => return,
            },
            Some(_) => return,
        };
        if self.cwd.is_empty() {
            self.cwd = s(&v, "cwd").to_string();
        }
        match s(&v, "type") {
            "assistant" => {
                self.touch(at);
                self.parse_assistant(&v, at);
            }
            "user" => {
                self.touch(at);
                self.parse_user(&v, at);
            }
            "ai-title" => {
                let t = s(&v, "aiTitle");
                if !t.is_empty() {
                    self.title = t.to_string();
                }
            }
            _ => {}
        }
    }

    fn touch(&mut self, t: i64) {
        if t == 0 {
            return;
        }
        if self.first_at == 0 || t < self.first_at {
            self.first_at = t;
        }
        if t > self.last_at {
            self.last_at = t;
        }
    }

    fn parse_assistant(&mut self, v: &Value, at: i64) {
        let m = v.get("message").unwrap_or(&Value::Null);
        let (mid, model) = (s(m, "id"), s(m, "model"));
        let real_model = !model.is_empty() && !model.starts_with('<'); // skip "<synthetic>"
        if real_model {
            self.model = model.to_string();
        }
        if !mid.is_empty() {
            let usage = m.get("usage").filter(|u| !u.is_null());
            if !self.usage.contains_key(mid) || usage.is_some() {
                let n = |k: &str| usage.and_then(|u| u.get(k)).and_then(Value::as_i64).unwrap_or(0);
                let u = Usage {
                    input: n("input_tokens"),
                    output: n("output_tokens"),
                    cache_read: n("cache_read_input_tokens"),
                    cache_write: n("cache_creation_input_tokens"),
                };
                self.usage.insert(mid.to_string(), u); // same message.id on several lines: counted once
            }
            if real_model {
                self.msg_model.insert(mid.to_string(), model.to_string());
            }
        }
        for c in m.get("content").and_then(Value::as_array).into_iter().flatten() {
            match s(c, "type") {
                "tool_use" => {
                    let (name, input) = (s(c, "name"), c.get("input").unwrap_or(&Value::Null));
                    let sum = tool_summary(name, input, 50);
                    self.last_tool_id = s(c, "id").to_string();
                    self.last_tool = tool_summary(name, input, NOW_MAX);
                    self.add_event(Event { at, kind: "tool_use", tool: name.into(), summary: sum });
                    self.record_interaction(at, name, s(c, "id"), input);
                }
                "text" => {
                    let t = one_line(s(c, "text"), 60);
                    if !t.is_empty() {
                        self.add_event(Event { at, kind: "text", tool: String::new(), summary: t });
                    }
                }
                _ => {}
            }
        }
    }

    fn record_interaction(&mut self, at: i64, name: &str, id: &str, input: &Value) {
        match name {
            "Agent" | "Task" => {
                let sp = Spawn { at, description: s(input, "description").into(), name: s(input, "name").into() };
                self.spawns.insert(id.to_string(), sp);
            }
            "SendMessage" => {
                let to = s(input, "to");
                if to.is_empty() {
                    return;
                }
                let mut label = s(input, "summary");
                if label.is_empty() {
                    label = s(input, "message"); // only when message is a string
                }
                self.sends.push(Sent { at, to: to.into(), label: one_line(label, 40) });
            }
            _ => {}
        }
    }

    fn parse_user(&mut self, v: &Value, at: i64) {
        let content = v.get("message").and_then(|m| m.get("content"));
        let mut texts: Vec<&str> = Vec::new();
        if let Some(t) = content.and_then(Value::as_str).filter(|t| !t.is_empty()) {
            texts.push(t);
        }
        for it in content.and_then(Value::as_array).into_iter().flatten() {
            match s(it, "type") {
                "tool_result" => {
                    let id = s(it, "tool_use_id");
                    if !id.is_empty() {
                        self.resolved.insert(id.to_string());
                    }
                }
                "text" => texts.push(s(it, "text")),
                _ => {}
            }
        }
        for t in texts {
            if let Some(from) = agent_message_from(t) {
                self.handbacks.push(Sent { at, to: from.into(), label: String::new() });
                self.add_event(Event { at, kind: "handback", tool: String::new(), summary: format!("agent-message from {from}") });
                continue;
            }
            if t.trim().starts_with('<') {
                continue; // system reminders, command wrappers, notifications
            }
            let t = one_line(t, 60);
            if !t.is_empty() {
                self.add_event(Event { at, kind: "prompt", tool: String::new(), summary: t });
            }
        }
    }

    fn add_event(&mut self, e: Event) {
        self.events.push(e);
        if self.events.len() > MAX_EVENTS {
            self.events.drain(..self.events.len() - MAX_EVENTS);
        }
    }
}

/// A byte-level filter run before any JSON decoding: false only when no
/// "type" key in b has an assistant, user or ai-title value.
fn may_be_relevant(mut b: &[u8]) -> bool {
    const KEY: &[u8] = b"\"type\"";
    const TYPES: [&[u8]; 3] = [b"\"assistant\"", b"\"user\"", b"\"ai-title\""];
    let trim = |b: &[u8]| -> usize { b.iter().take_while(|&&c| c == b' ' || c == b'\t').count() };
    loop {
        let Some(i) = b.windows(KEY.len()).position(|w| w == KEY) else { return false };
        b = &b[i + KEY.len()..];
        b = &b[trim(b)..];
        if b.first() != Some(&b':') {
            continue;
        }
        let v = &b[1..];
        let v = &v[trim(v)..];
        if TYPES.iter().any(|t| v.starts_with(t)) {
            return true;
        }
    }
}

/// The sender id of the first `<agent-message from="<id>"` in t.
fn agent_message_from(t: &str) -> Option<&str> {
    const OPEN: &str = "<agent-message from=\"";
    let mut rest = t;
    while let Some(i) = rest.find(OPEN) {
        rest = &rest[i + OPEN.len()..];
        let n = rest.bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-').count();
        if n > 0 && rest.as_bytes().get(n) == Some(&b'"') {
            return Some(&rest[..n]);
        }
    }
    None
}

/// How much of the running tool call the card's "now:" may show (3 lines).
const NOW_MAX: usize = 150;

/// "Tool: arg" from a tool_use input, the arg's lines joined and cut to max.
fn tool_summary(name: &str, input: &Value, max: usize) -> String {
    let arg = match name {
        "Bash" => s(input, "command").to_string(),
        "Read" | "Write" | "Edit" | "NotebookEdit" => short_path(s(input, "file_path")),
        "Agent" | "Task" => {
            let (d, t) = (s(input, "description"), s(input, "subagent_type"));
            if t.is_empty() { d.to_string() } else { format!("{d} ({t})") }
        }
        "SendMessage" => format!("to {}", s(input, "to")),
        _ => ["description", "file_path", "path", "pattern", "query", "skill", "url", "name"]
            .iter()
            .map(|k| s(input, k))
            .find(|v| !v.is_empty())
            .unwrap_or("")
            .to_string(),
    };
    let arg = truncate(&arg.split_whitespace().collect::<Vec<_>>().join(" "), max);
    if arg.is_empty() { name.to_string() } else { format!("{name}: {arg}") }
}

/// Keeps the last two path segments.
fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.trim_end_matches('/').split('/').collect();
    if parts.len() <= 3 {
        return p.to_string();
    }
    format!("…/{}", parts[parts.len() - 2..].join("/"))
}

/// The first non-empty line of s, trimmed and cut to max characters.
pub fn one_line(s: &str, max: usize) -> String {
    s.split('\n').map(str::trim).find(|l| !l.is_empty()).map_or_else(String::new, |l| truncate(l, max))
}

/// Cuts s to at most max characters, marking the cut with "…".
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return s.chars().take(max).collect();
    }
    s.chars().take(max - 1).chain(['…']).collect()
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix nanoseconds of an RFC 3339 time such as 2026-01-05T10:20:30.123Z.
pub fn parse_rfc3339(t: &str) -> Option<i64> {
    let b = t.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let d = b.get(r)?;
        d.iter().all(u8::is_ascii_digit).then(|| d.iter().fold(0, |n, c| n * 10 + i64::from(c - b'0')))
    };
    let sep = |i: usize, c: u8| b.get(i) == Some(&c);
    if !(sep(4, b'-') && sep(7, b'-') && sep(10, b'T') && sep(13, b':') && sep(16, b':')) {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let mut i = 19;
    let mut nanos = 0;
    if sep(i, b'.') {
        let n = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return None;
        }
        for k in 0..9 {
            nanos = nanos * 10 + if k < n { i64::from(b[i + 1 + k] - b'0') } else { 0 };
        }
        i += 1 + n;
    }
    let offset = match b.get(i) {
        Some(b'Z') if i + 1 == b.len() => 0,
        Some(&c @ (b'+' | b'-')) if i + 6 == b.len() && sep(i + 3, b':') => {
            let o = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
            if c == b'-' { -o } else { o }
        }
        _ => return None,
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset;
    Some(secs * SEC + nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rfc3339_parses_zulu_offsets_and_fractions() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:01Z"), Some(SEC));
        assert_eq!(parse_rfc3339("1970-01-01T07:00:00+07:00"), Some(0));
        assert_eq!(parse_rfc3339("2026-01-05T10:20:30.5Z"), Some((days_from_civil(2026, 1, 5) * 86_400 + 37_230) * SEC + SEC / 2));
        assert_eq!(parse_rfc3339("2026-01-05 10:20:30Z"), None);
        assert_eq!(parse_rfc3339("2026-13-05T10:20:30Z"), None);
    }

    #[test]
    fn filter_summaries_and_handbacks() {
        assert!(may_be_relevant(br#"{"x":1,"type" : "user"}"#));
        assert!(!may_be_relevant(br#"{"type":"progress","data":{"type":"x"}}"#));
        assert_eq!(agent_message_from(r#"hi <agent-message from="" x <agent-message from="a-1_b">"#), Some("a-1_b"));
        assert_eq!(short_path("/a/b/c/d.rs"), "…/c/d.rs");
        assert_eq!(short_path("b/c.rs"), "b/c.rs");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(one_line("\n  first line \nsecond", 60), "first line");
        let v: Value = serde_json::from_str(r#"{"description":"look","subagent_type":"Explore"}"#).unwrap();
        assert_eq!(tool_summary("Agent", &v, 50), "Agent: look (Explore)");
        assert_eq!(tool_summary("Grep", &Value::Null, 50), "Grep");
    }

    #[test]
    fn root_labels_name_the_config_dir() {
        assert_eq!(Root::new("/h/.claude/projects".into()).label, "claude");
        assert_eq!(Root::new("/h/.claude-personal/projects".into()).label, "personal");
        assert_eq!(Root::new("/tmp/fixture".into()).label, "fixture");
    }

    fn write(path: &Path, lines: &[&str]) {
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
}
