//! Parses one transcript, incrementally, into the state an agent is built from.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde_json::Value;


use super::{
    ACTIVE_WINDOW, Agent, Event, Kind, MAX_EVENTS, MAX_TEXT, RUNNING_WINDOW, Status, Usage, WAIT_QUIET, WAIT_WINDOW,
    text::{NOW_MAX, asking, one_line, parse_rfc3339, s, tool_summary, truncate},
};

/// The subset of agent-<id>.meta.json the monitor uses.
#[derive(Clone, Debug, Default)]
pub(super) struct Meta {
    pub(super) agent_type: String,
    pub(super) description: String,
    pub(super) tool_use_id: String,
    pub(super) finished: bool,
}

/// An Agent/Task tool_use seen in a transcript.
#[derive(Clone, Debug, Default)]
pub(super) struct Spawn {
    pub(super) at: i64,
    pub(super) description: String,
    pub(super) name: String, // optional input.name, resolves SendMessage targets
}

/// A SendMessage tool_use, or a handback notice (`to` is then the sender).
#[derive(Clone, Debug)]
pub(super) struct Sent {
    pub(super) at: i64,
    pub(super) to: String,
    pub(super) label: String,
}

/// The parsed state of one transcript, advanced incrementally.
#[derive(Debug, Default)]
pub(super) struct FileState {
    pub(super) offset: u64, // bytes consumed: always ends just after a '\n'
    pub(super) title: String,
    model: String,
    pub(super) cwd: String,
    pub(super) first_at: i64,
    last_at: i64,
    usage: HashMap<String, Usage>,      // per message.id, so repeated lines count once
    msg_model: HashMap<String, String>, // message.id -> model
    events: Vec<Event>,
    last_tool_id: String,
    last_tool_name: String,
    last_tool: String,
    last_text: String,
    asking: String, // of the last tool_use: what it asks the user, if anything
    continued: bool, // the conversation moved on to another session id (`/resume`, `/clear`)
    pub(super) mode: String, // the last permission-mode entry: "default", "plan", "acceptEdits", "auto", ...
    resolved: HashSet<String>,     // tool_use ids that have a tool_result
    pub(super) spawns: HashMap<String, Spawn>, // tool_use id -> Agent/Task call
    pub(super) sends: Vec<Sent>,
    pub(super) handbacks: Vec<Sent>,
}

pub(super) fn read_meta(path: &Path) -> Meta {
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
/// Whether a call of tool stops for the user in permission mode: a question
/// or a plan always does; in a mode that asks, every tool that changes
/// something or reaches out (in acceptEdits, all but file edits). Rules the
/// user allowed in settings are not known here, so those count as asking.
fn asks_user(mode: &str, tool: &str) -> bool {
    const NEVER_ASK: [&str; 9] = ["Read", "Glob", "Grep", "LS", "TodoWrite", "ToolSearch", "Agent", "Task", "SendMessage"];
    const EDITS: [&str; 4] = ["Edit", "MultiEdit", "Write", "NotebookEdit"];
    match tool {
        "AskUserQuestion" | "ExitPlanMode" => true,
        _ if NEVER_ASK.contains(&tool) => false,
        _ => match mode {
            "default" | "plan" => true,
            "acceptEdits" => !EDITS.contains(&tool),
            _ => false, // auto, bypassPermissions, dontAsk, or not recorded
        },
    }
}

pub(super) fn build(st: &FileState, id: String, mtime: i64, now: i64, finished: bool, mode: &str) -> Agent {
    let mut a = Agent {
        id,
        parent_id: String::new(),
        title: String::new(),
        agent_type: String::new(),
        model: st.model.clone(),
        project: String::new(),
        session_id: String::new(),
        config: String::new(),
        config_dir: PathBuf::new(),
        cwd: String::new(),
        kind: Kind::Session,
        status: Status::Idle,
        last_activity: if st.last_at == 0 { mtime } else { st.last_at },
        last_tool: st.last_tool.clone(),
        last_text: st.last_text.clone(),
        asking: if st.pending() { st.asking.clone() } else { String::new() },
        turns: st.usage.len(),
        usage: Usage::default(),
        model_usage: BTreeMap::new(),
        events: st.events.clone(),
        children: Vec::new(),
        live: false,
        continued: st.continued,
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
    } else if st.pending() && (WAIT_QUIET..WAIT_WINDOW).contains(&age) && asks_user(mode, &st.last_tool_name) {
        Status::Waiting
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
pub(super) fn project_name(cwd: &str, dir: &str) -> String {
    if !cwd.is_empty() {
        let t = cwd.trim_end_matches('/');
        return if t.is_empty() { "/".into() } else { t.rsplit('/').next().unwrap_or(t).into() };
    }
    match dir.rfind('-') {
        Some(i) if i < dir.len() - 1 => dir[i + 1..].into(),
        _ => dir.into(),
    }
}

impl FileState {
    /// Whether the most recent tool_use still waits for its result.
    fn pending(&self) -> bool {
        !self.last_tool_id.is_empty() && !self.resolved.contains(&self.last_tool_id)
    }

    /// Consumes complete lines from the offset to EOF. A trailing line
    /// without '\n' is left for the next call.
    pub(super) fn read_from(&mut self, path: &Path) -> std::io::Result<()> {
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
            "permission-mode" => self.mode = s(&v, "permissionMode").to_string(),
            "continued-in" => self.continued = true,
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
                    self.last_tool_name = name.to_string();
                    self.asking = truncate(&asking(name, input), MAX_TEXT);
                    self.last_tool = tool_summary(name, input, NOW_MAX);
                    self.add_event(Event { at, kind: "tool_use", tool: name.into(), summary: sum, ..Event::default() });
                    self.record_interaction(at, name, s(c, "id"), input);
                }
                "text" => {
                    if !s(c, "text").trim().is_empty() {
                        self.last_text = truncate(s(c, "text").trim(), MAX_TEXT);
                    }
                    let t = one_line(s(c, "text"), 60);
                    if !t.is_empty() {
                        let text = truncate(s(c, "text").trim(), MAX_TEXT);
                        self.add_event(Event { at, kind: "text", summary: t, text, ..Event::default() });
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
            let sent = typed(t);
            let t = sent.as_deref().unwrap_or(t);
            if let Some(from) = agent_message_from(t) {
                self.handbacks.push(Sent { at, to: from.into(), label: String::new() });
                self.add_event(Event { at, kind: "handback", summary: format!("agent-message from {from}"), ..Event::default() });
                continue;
            }
            if t.trim().starts_with('<') {
                continue; // system reminders, command wrappers, notifications
            }
            let summary = one_line(t, 60);
            if !summary.is_empty() {
                self.add_event(Event { at, kind: "prompt", summary, text: truncate(t.trim(), MAX_TEXT), ..Event::default() });
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
    const TYPES: [&[u8]; 5] = [b"\"assistant\"", b"\"user\"", b"\"ai-title\"", b"\"permission-mode\"", b"\"continued-in\""];
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

/// What was typed, from a prompt office's old prompt panel framed for a
/// session: as it reaches the transcript, wrapped in a hook's system reminder.
pub fn typed(t: &str) -> Option<String> {
    const TEXT: &str = "from claude-monitor; treat it as their next message:\n\n";
    const COMMAND: &str = "The user ran the slash command `";
    if let Some(i) = t.find(COMMAND) {
        let rest = &t[i + COMMAND.len()..];
        return Some(rest[..rest.find('`')?].to_string());
    }
    let rest = &t[t.find(TEXT)? + TEXT.len()..];
    let end = rest.find("\n\n</system-reminder>").or_else(|| rest.find("</system-reminder>")).unwrap_or(rest.len());
    Some(rest[..end].trim().to_string()).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::super::{Root, SEC, Scanner, mtime, tests::write, text::short_path};
    use super::*;

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

    /// A prompt sent from office arrives as a hook's system reminder; the
    /// conversation shows what was typed, a command as typed.
    #[test]
    fn a_prompt_from_office_reads_as_typed() {
        let wrapped = |m: &str| format!("<task-notification>\n<summary>Stop hook feedback</summary>\n</task-notification>\n<system-reminder>\nStop hook blocking error from command \"Stop\": {m}\n\n</system-reminder>");
        let text = wrapped("The user sent this prompt from claude-monitor; treat it as their next message:\n\nfix the build\nthen test");
        assert_eq!(typed(&text).as_deref(), Some("fix the build\nthen test"));
        let cmd = wrapped("The user ran the slash command `/pstack:poteto-mode fix it` from claude-monitor; treat it as their next message, typed as a command. Read /x/SKILL.md in full");
        assert_eq!(typed(&cmd).as_deref(), Some("/pstack:poteto-mode fix it"));
        assert_eq!(typed("<system-reminder>something else</system-reminder>"), None);
        let mut st = FileState::default();
        st.parse_user(&serde_json::json!({"message": {"content": text}}), 1);
        assert_eq!((st.events[0].kind, st.events[0].text.as_str()), ("prompt", "fix the build\nthen test"));
    }

    /// A tool call that needs the user, unanswered and quiet for a few
    /// seconds, is Waiting: always for a question or a plan, else by the
    /// session's permission mode, which subagents share.
    #[test]
    fn waiting_follows_the_tool_and_the_sessions_mode() {
        let pending = |tool: &str| FileState { last_tool_id: "t".into(), last_tool_name: tool.into(), ..FileState::default() };
        let at = |st: &FileState, mode: &str, age: i64| build(st, "x".into(), 0, age, false, mode).status;
        for (tool, mode, want) in [
            ("Bash", "default", Status::Waiting),
            ("Bash", "plan", Status::Waiting),
            ("Bash", "auto", Status::Running),
            ("Bash", "bypassPermissions", Status::Running),
            ("Bash", "", Status::Running),
            ("Edit", "acceptEdits", Status::Running),
            ("Bash", "acceptEdits", Status::Waiting),
            ("Read", "default", Status::Running),
            ("Agent", "plan", Status::Running),
            ("AskUserQuestion", "auto", Status::Waiting),
            ("ExitPlanMode", "bypassPermissions", Status::Waiting),
        ] {
            assert_eq!(at(&pending(tool), mode, 5 * SEC), want, "{tool} in {mode:?}");
        }
        assert_eq!(at(&pending("Bash"), "default", SEC), Status::Running, "not quiet long enough yet");
        assert_eq!(at(&pending("Bash"), "default", 2 * WAIT_WINDOW), Status::Idle, "long gone");
        let mut answered = pending("AskUserQuestion");
        answered.resolved.insert("t".into());
        assert_eq!(at(&answered, "default", 5 * SEC), Status::Active);

        let dir = std::env::temp_dir().join(format!("office-wait-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let proj = dir.join("projects").join("-home-me-proj");
        write(&proj.join("s1.jsonl"), &[
            r#"{"type":"permission-mode","permissionMode":"auto","sessionId":"s1"}"#,
            r#"{"type":"permission-mode","permissionMode":"default","sessionId":"s1"}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"tu1","name":"Agent","input":{"description":"d"}}]}}"#,
        ]);
        write(&proj.join("s1").join("subagents").join("agent-a1.jsonl"), &[
            r#"{"type":"assistant","timestamp":"2026-01-01T10:00:01Z","message":{"id":"x1","content":[{"type":"tool_use","id":"tu2","name":"Bash","input":{"command":"rm -rf build"}}]}}"#,
        ]);
        let now = mtime(&fs::metadata(proj.join("s1").join("subagents").join("agent-a1.jsonl")).unwrap()) + 5 * SEC;
        let snap = Scanner::new(vec![Root::new(dir.join("projects"))], 0).scan(now);
        assert_eq!(snap.by_id["a1"].status, Status::Waiting, "the subagent runs in its session's last mode");
        assert_eq!(snap.by_id["s1"].status, Status::Running, "spawning asks no one");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A pending question keeps its questions and numbered options, a plan
    /// its text, and the reply before them is kept whole; answered, nothing is asked.
    #[test]
    fn keeps_the_reply_and_what_a_pending_call_asks() {
        let mut st = FileState::default();
        st.parse_line(br#"{"type":"assistant","timestamp":"2026-01-01T10:00:00Z","message":{"id":"m1","content":[{"type":"text","text":"I found two ways.\nWhich do you want?"}]}}"#);
        st.parse_line(br#"{"type":"assistant","timestamp":"2026-01-01T10:00:01Z","message":{"id":"m2","content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which approach?","options":[{"label":"Fast","description":"skip the cache"},{"label":"Safe"}]}]}}]}}"#);
        let a = build(&st, "x".into(), 0, 5 * SEC, false, "default");
        assert_eq!(a.last_text, "I found two ways.\nWhich do you want?");
        assert_eq!(a.asking, "Which approach?\n  1. Fast — skip the cache\n  2. Safe");
        assert_eq!(a.last_tool, "AskUserQuestion: Which approach?");
        st.parse_line(br#"{"type":"user","timestamp":"2026-01-01T10:00:09Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1","content":"Safe"}]}}"#);
        assert_eq!(build(&st, "x".into(), 0, 5 * SEC, false, "default").asking, "", "answered");
        st.parse_line(br#"{"type":"assistant","timestamp":"2026-01-01T10:00:10Z","message":{"id":"m3","content":[{"type":"tool_use","id":"p1","name":"ExitPlanMode","input":{"plan":"1. Add the cache\n2. Test it"}}]}}"#);
        let a = build(&st, "x".into(), 0, 5 * SEC, false, "plan");
        assert_eq!((a.asking.as_str(), a.last_tool.as_str()), ("1. Add the cache\n2. Test it", "ExitPlanMode: approve the plan"));
    }
}
