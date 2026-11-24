//! The bridge between office and running Claude Code sessions, through
//! Claude Code's own hooks, so it works in any terminal (or none).
//!
//! office installs two hooks (`office hook install`), both this binary:
//! - PermissionRequest runs `office hook permission`. While office runs, it
//!   files the request as ask-<pid>.json and waits up to HOLD for office to
//!   answer it (answer-<pid>.json): allow, allow and don't ask again, or deny.
//!   Claude Code shows its own dialog meanwhile; answered there, the call's
//!   result reaches the transcript and the hook lets go. When office is not
//!   running, the hook returns at once.
//! - Stop and SessionStart run `office hook wait` as an asyncRewake hook: it
//!   waits in the background, never blocking the session, until office
//!   leaves a prompt (prompt-<nanos>.txt, the text the session reads, which office
//!   frames: see commands::message), then exits 2, which wakes the session with
//!   it. One waiter per session: a newer one retires the older.
//!
//! Files live in $XDG_STATE_HOME/claude-monitor/bridge (else
//! ~/.local/state/claude-monitor/bridge), one directory per session id, each
//! file with one writer and written whole (to a temporary name, then renamed).
//! `alive` is office's heartbeat: the hooks treat office as gone once it is
//! older than STALE.

use std::{
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process,
    thread,
    time::{Duration, SystemTime},
};

use serde_json::{Value, json};

/// How long a permission request stays answerable from office. Claude Code's
/// own dialog is up all along, so waiting costs the session nothing.
pub const HOLD: Duration = Duration::from_secs(600);
/// office counts as gone once its heartbeat is this old.
const STALE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(150);
/// A waiter gives up after this long, so none outlives a forgotten session.
const WAITER_LIFE: Duration = Duration::from_secs(24 * 3600);
/// What every command office installs starts with, after the binary's path.
const MARK: &str = " hook ";

/// A permission request waiting for an answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub id: String,      // the hook's pid: names its files
    pub summary: String, // the tool call, as the tree shows tool calls
    pub at: SystemTime,  // when it was filed
    pub questions: Vec<Question>, // AskUserQuestion's: answered by picking, not by yes or no
    pub detail: Vec<String>, // the whole call, a line each: see detail()
    pub rules: Vec<String>,  // what "don't ask again" saves: see rules()
}

/// One question of an AskUserQuestion call.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub text: String,
    pub options: Vec<(String, String)>, // label, description
    pub multi: bool,                    // several may be picked (answers comma-separated)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Allow,
    AllowAlways, // allow, and add Claude Code's suggested rule so it asks no more
    Deny,
    Answers(Vec<(String, String)>), // AskUserQuestion: question text, answer
}

impl Answer {
    /// The answer file's content.
    fn body(&self) -> String {
        match self {
            Answer::Allow => "allow".into(),
            Answer::AllowAlways => "always".into(),
            Answer::Deny => "deny".into(),
            Answer::Answers(a) => {
                let map: serde_json::Map<String, Value> = a.iter().map(|(q, a)| (q.clone(), Value::from(a.as_str()))).collect();
                json!({ "answers": map }).to_string()
            }
        }
    }
}

/// AskUserQuestion's questions, from its input.
fn questions(input: &Value) -> Vec<Question> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|q| Question {
            text: s(q, "question"),
            options: q.get("options").and_then(Value::as_array).into_iter().flatten().map(|o| (s(o, "label"), s(o, "description"))).collect(),
            multi: q.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
        })
        .collect()
}

pub fn dir() -> PathBuf {
    let home = || env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let state = env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()).map_or_else(|| home().join(".local").join("state"), PathBuf::from);
    state.join("claude-monitor").join("bridge")
}

/// The directory of one session; the id as Claude Code knows it (no "@config").
fn session_dir(root: &Path, session: &str) -> PathBuf {
    root.join(session.split('@').next().unwrap_or(session))
}

/// Writes path whole: readers see the old content or the new, never a part.
fn put(path: &Path, body: &[u8]) -> io::Result<()> {
    fs::create_dir_all(path.parent().expect("bridge files sit in a directory"))?;
    let tmp = path.with_extension(format!("tmp{}", process::id()));
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)
}

fn age(path: &Path) -> Option<Duration> {
    let t = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(SystemTime::now().duration_since(t).unwrap_or_default())
}

// office's side.

/// Marks office as running; call it every second or so.
pub fn heartbeat(root: &Path) {
    let _ = put(&root.join("alive"), b"");
}

/// Marks office as gone, so waiting permission requests show their dialog at once.
pub fn leave(root: &Path) {
    let _ = fs::remove_file(root.join("alive"));
}

/// Whether the hook that filed ask-<id>.json still waits for its answer: a
/// hook killed with its session leaves its file behind.
fn hook_alive(id: &str) -> bool {
    !cfg!(unix) || id.parse().is_ok_and(alive_pid)
}

/// The oldest permission request of session waiting for an answer.
pub fn ask(root: &Path, session: &str) -> Option<Ask> {
    let d = session_dir(root, session);
    let mut asks: Vec<Ask> = fs::read_dir(&d)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_prefix("ask-")?.strip_suffix(".json")?.to_string();
            if !hook_alive(&id) {
                return None;
            }
            let v: Value = serde_json::from_slice(&fs::read(e.path()).ok()?).ok()?;
            let at = e.metadata().and_then(|m| m.modified()).ok()?;
            let qs = v.get("questions").map(|q| questions(&json!({ "questions": q }))).unwrap_or_default();
            let tool = v.get("tool").and_then(Value::as_str).unwrap_or("");
            let (detail, rules) = (detail(tool, v.get("input").unwrap_or(&Value::Null)), rules(v.get("suggestions").unwrap_or(&Value::Null)));
            Some(Ask { id, summary: v.get("summary")?.as_str()?.to_string(), at, questions: qs, detail, rules })
        })
        .collect();
    asks.sort_by_key(|a| a.at);
    asks.into_iter().next()
}

pub fn answer(root: &Path, session: &str, ask: &Ask, a: Answer) -> io::Result<()> {
    put(&session_dir(root, session).join(format!("answer-{}.json", ask.id)), a.body().as_bytes())
}

/// The prompts left in d and not taken yet, oldest first: one file each,
/// prompt-<unix nanos>.txt, so no send replaces another.
fn prompts(d: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("prompt-") && n.ends_with(".txt")))
        .collect();
    out.sort();
    out
}

/// Removes the prompt at p and gives its text, unless the other side
/// removed it first (the waiter took it, or office took it back).
fn claim(p: &Path) -> Option<String> {
    let m = fs::read_to_string(p).ok()?;
    fs::remove_file(p).ok()?;
    Some(m)
}

/// Leaves a prompt for the session's waiter, after any not taken yet. Says
/// whether a waiter is there to take it now.
pub fn send(root: &Path, session: &str, prompt: &str) -> io::Result<bool> {
    let d = session_dir(root, session);
    let at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
    put(&d.join(format!("prompt-{at:020}.txt")), prompt.as_bytes())?;
    let waiter = fs::read_to_string(d.join("waiter")).ok().and_then(|p| p.trim().parse::<i64>().ok());
    Ok(waiter.is_some_and(|p| !cfg!(unix) || alive_pid(p)))
}

/// Takes back the newest prompt left for the session and not taken yet:
/// Ok(None) when there is none left.
pub fn cancel(root: &Path, session: &str) -> io::Result<Option<String>> {
    Ok(prompts(&session_dir(root, session)).iter().rev().find_map(|p| claim(p)))
}

/// The oldest prompt left in d, taken: what the waiter wakes the session with.
fn take(d: &Path) -> Option<String> {
    prompts(d).iter().find_map(|p| claim(p))
}

/// The prompts left for the session and not taken yet, oldest first.
pub fn queued(root: &Path, session: &str) -> Vec<String> {
    prompts(&session_dir(root, session)).iter().filter_map(|p| fs::read_to_string(p).ok()).collect()
}

// The hooks' side: `office hook permission` and `office hook wait`, with the
// hook's JSON on stdin.

/// Whether the session runs headless (`claude -p`, the Agent SDK), going by
/// the CLAUDE_CODE_ENTRYPOINT its hooks see: "sdk-cli", "sdk-ts", ... Such a
/// run ends on its own, and waits for its async hooks first: a waiter would
/// hang it, and a held request would stall it, so the hooks do nothing there.
fn headless(entrypoint: Option<&str>) -> bool {
    entrypoint.is_some_and(|e| e.starts_with("sdk"))
}

/// Runs the hook named kind and returns its exit code.
pub fn hook(kind: &str, root: &Path) -> i32 {
    if headless(env::var("CLAUDE_CODE_ENTRYPOINT").ok().as_deref()) {
        return 0;
    }
    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);
    let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let session = v.get("session_id").and_then(Value::as_str).unwrap_or("");
    if session.is_empty() {
        return 0;
    }
    let d = session_dir(root, session);
    match kind {
        "permission" => {
            if let Some(out) = permission(&d, root, &v) {
                println!("{out}");
            }
            0
        }
        "wait" => match wait(&d) {
            Some(prompt) => {
                eprintln!("{prompt}");
                2
            }
            None => 0,
        },
        _ => 0,
    }
}

fn alive(root: &Path) -> bool {
    age(&root.join("alive")).is_some_and(|a| a < STALE)
}

/// Files the request and waits for office's answer: the hook's output, or
/// None to leave it to Claude Code's dialog.
fn permission(d: &Path, root: &Path, v: &Value) -> Option<String> {
    if !alive(root) {
        return None;
    }
    let tool = v.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let summary = crate::scan::tool_summary(tool, v.get("tool_input").unwrap_or(&Value::Null), 200);
    let id = process::id().to_string();
    let (ask, ans) = (d.join(format!("ask-{id}.json")), d.join(format!("answer-{id}.json")));
    let qs = if tool == "AskUserQuestion" { v.get("tool_input").and_then(|i| i.get("questions")).cloned() } else { None };
    let agent = v.get("agent_id").and_then(Value::as_str).unwrap_or("");
    let (input, suggestions) = (v.get("tool_input").cloned().unwrap_or(Value::Null), v.get("permission_suggestions").cloned().unwrap_or(Value::Null));
    let filed = json!({ "summary": summary, "questions": qs, "agent": agent, "tool": tool, "input": input, "suggestions": suggestions });
    put(&ask, filed.to_string().as_bytes()).ok()?;
    // Claude Code shows its own dialog meanwhile and leaves this hook running
    // when it is answered there: the call's tool_result in the transcript of
    // the agent that asked says so.
    let mut watch = transcript(v).map(|p| (fs::metadata(&p).map_or(0, |m| m.len()), p));
    let start = SystemTime::now();
    let got = loop {
        if let Ok(a) = fs::read_to_string(&ans) {
            break Some(a);
        }
        if !alive(root) || start.elapsed().unwrap_or_default() >= HOLD {
            break None;
        }
        if let Some((from, p)) = &mut watch
            && let Ok(b) = fs::read(&*p)
            && b.len() as u64 > *from
        {
            if b[*from as usize..].windows(13).any(|w| w == b"\"tool_result\"") {
                break None;
            }
            *from = b.len() as u64;
        }
        thread::sleep(POLL);
    };
    let _ = fs::remove_file(&ask);
    let _ = fs::remove_file(&ans);
    decision(got?.trim(), v)
}

/// The transcript of the agent a hook fires for: the session's, or a
/// subagent's beside it (<session>/subagents/agent-<agent_id>.jsonl).
fn transcript(v: &Value) -> Option<PathBuf> {
    let session = PathBuf::from(v.get("transcript_path")?.as_str()?);
    match v.get("agent_id").and_then(Value::as_str) {
        Some(agent) => Some(session.with_extension("").join("subagents").join(format!("agent-{agent}.jsonl"))),
        None => Some(session),
    }
}

/// The whole of a tool call a permission request is for, a line each, as
/// the panel shows it: a command as written; an edit's file, then its old
/// lines after `- ` and new ones after `+ `; a write's file, then its lines;
/// anything else as `key: value`.
pub fn detail(tool: &str, input: &Value) -> Vec<String> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let lines = |prefix: &str, text: &str| text.lines().map(|l| format!("{prefix}{l}")).collect::<Vec<_>>();
    let edit = |e: &Value| [lines("- ", &s(e, "old_string")), lines("+ ", &s(e, "new_string"))].concat();
    match tool {
        "Bash" => lines("", &s(input, "command")),
        "Edit" => [vec![s(input, "file_path")], edit(input)].concat(),
        "MultiEdit" => {
            let edits = input.get("edits").and_then(Value::as_array).into_iter().flatten();
            std::iter::once(s(input, "file_path")).chain(edits.flat_map(edit)).collect()
        }
        "Write" => [vec![s(input, "file_path")], lines("+ ", &s(input, "content"))].concat(),
        _ => input
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| format!("{k}: {}", v.as_str().map_or_else(|| v.to_string(), str::to_string)))
            .collect(),
    }
}

/// What answering "yes, don't ask again" saves, from the request's
/// permission_suggestions: `Bash(npm test:*) (this project)`, a folder it
/// may use, or a permission mode, each with where it is saved.
pub fn rules(suggestions: &Value) -> Vec<String> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut out = Vec::new();
    for u in suggestions.as_array().into_iter().flatten() {
        let place = match s(u, "destination").as_str() {
            "session" => " (this session)",
            "localSettings" => " (this project, just you)",
            "projectSettings" => " (this project)",
            "userSettings" => " (all projects)",
            _ => "",
        };
        let what: Vec<String> = match s(u, "type").as_str() {
            "addRules" | "replaceRules" => u.get("rules").and_then(Value::as_array).into_iter().flatten().map(|r| match s(r, "ruleContent") {
                c if c.is_empty() => s(r, "toolName"),
                c => format!("{}({c})", s(r, "toolName")),
            }).collect(),
            "addDirectories" => u.get("directories").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(|d| format!("folder {}", tilde(d))).collect(),
            "setMode" => vec![format!("mode {}", s(u, "mode"))],
            t => vec![t.to_string()],
        };
        out.extend(what.into_iter().filter(|w| !w.is_empty()).map(|w| format!("{w}{place}")));
    }
    out
}

/// The PermissionRequest hook's output for office's answer.
/// For AskUserQuestion, the picked answers go into its input, as its own dialog does.
fn decision(answer: &str, v: &Value) -> Option<String> {
    let suggestions = v.get("permission_suggestions");
    let d = match answer {
        a if a.starts_with('{') => {
            let answers = serde_json::from_str::<Value>(a).ok()?.get("answers")?.clone();
            let mut input = v.get("tool_input").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
            input.as_object_mut().expect("an object").insert("answers".into(), answers);
            json!({ "behavior": "allow", "updatedInput": input })
        }
        "allow" => json!({ "behavior": "allow" }),
        "always" => match suggestions.filter(|s| s.as_array().is_some_and(|a| !a.is_empty())) {
            Some(s) => json!({ "behavior": "allow", "updatedPermissions": s }),
            None => json!({ "behavior": "allow" }),
        },
        "deny" => json!({ "behavior": "deny", "message": "The user denied this from claude-monitor." }),
        _ => return None,
    };
    Some(json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": d } }).to_string())
}

/// Waits for a prompt for the session, as its only waiter: the prompt, or
/// None once a newer waiter took over, Claude Code is gone, or WAITER_LIFE passed.
fn wait(d: &Path) -> Option<String> {
    let me = process::id().to_string();
    let waiter = d.join("waiter");
    put(&waiter, me.as_bytes()).ok()?;
    let session = session_pid();
    let start = SystemTime::now();
    let mut beat = 0u32;
    let out = loop {
        if fs::read_to_string(&waiter).ok().as_deref() != Some(me.as_str()) {
            return None; // retired: the newer waiter owns the file
        }
        if let Some(p) = take(d) {
            break Some(p);
        }
        beat += 1;
        if start.elapsed().unwrap_or_default() >= WAITER_LIFE || (session > 0 && beat.is_multiple_of(20) && !alive_pid(session.into())) {
            break None;
        }
        thread::sleep(POLL);
    };
    if fs::read_to_string(&waiter).ok().as_deref() == Some(me.as_str()) {
        let _ = fs::remove_file(&waiter);
    }
    out
}

/// The Claude Code process this hook runs for. Claude Code runs a hook
/// command through a shell (`/bin/sh -c ...`), which outlives a killed
/// session while this process runs: the shell's parent is the session.
#[cfg(unix)]
fn session_pid() -> i32 {
    // SAFETY: getppid takes nothing and cannot fail.
    let parent = unsafe { libc::getppid() };
    let comm = fs::read_to_string(format!("/proc/{parent}/comm")).unwrap_or_default();
    if !matches!(comm.trim(), "sh" | "bash" | "dash" | "zsh") {
        return parent;
    }
    // /proc/<pid>/stat: "pid (comm) state ppid ...", comm in parentheses.
    let stat = fs::read_to_string(format!("/proc/{parent}/stat")).unwrap_or_default();
    stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)?.parse().ok()).unwrap_or(parent)
}
#[cfg(not(unix))]
fn session_pid() -> i32 {
    0 // not known: the waiter then lives until a prompt, a newer waiter, or WAITER_LIFE
}

// Installing the hooks into a Claude Code config directory's settings.json.

/// The hook entries office adds, run by exe.
fn entries(exe: &str) -> [(&'static str, Value); 3] {
    let cmd = |kind: &str| format!("\"{exe}\"{MARK}{kind}");
    let permission = json!({ "matcher": "*", "hooks": [{ "type": "command", "command": cmd("permission"), "timeout": HOLD.as_secs() + 30 }] });
    // Claude Code kills a hook after its timeout, async ones too (600 s by
    // default): the waiter must outlive an idle session's quiet hours.
    let wait = json!({ "hooks": [{ "type": "command", "command": cmd("wait"), "asyncRewake": true, "timeout": WAITER_LIFE.as_secs() }] });
    [("PermissionRequest", permission), ("Stop", wait.clone()), ("SessionStart", wait)]
}

/// Whether an entry of a hooks list runs office.
fn is_ours(entry: &Value) -> bool {
    entry.get("hooks").and_then(Value::as_array).into_iter().flatten().any(|h| {
        h.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(MARK) && (c.contains("office") || c.contains("office.exe")))
    })
}

/// settings with office's hooks removed, then (when exe is given) added again.
pub fn with_hooks(mut settings: Value, exe: Option<&str>) -> Value {
    if !settings.is_object() {
        settings = json!({});
    }
    let hooks = settings.as_object_mut().expect("made an object").entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks = hooks.as_object_mut().expect("made an object");
    for list in hooks.values_mut() {
        if let Some(a) = list.as_array_mut() {
            a.retain(|e| !is_ours(e));
        }
    }
    hooks.retain(|_, list| list.as_array().is_none_or(|a| !a.is_empty()));
    if let Some(exe) = exe {
        for (event, entry) in entries(exe) {
            let list = hooks.entry(event).or_insert_with(|| json!([]));
            if let Some(a) = list.as_array_mut() {
                a.push(entry);
            }
        }
    }
    if hooks.is_empty() {
        settings.as_object_mut().expect("made an object").remove("hooks");
    }
    settings
}

/// Adds (exe given) or removes office's hooks in config_dir/settings.json,
/// keeping a copy of the old file as settings.json.bak-office.
pub fn install(config_dir: &Path, exe: Option<&str>) -> io::Result<()> {
    let path = config_dir.join("settings.json");
    let old = fs::read(&path).ok();
    let settings = match &old {
        Some(b) => serde_json::from_slice(b).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?,
        None => json!({}),
    };
    let new = with_hooks(settings, exe);
    let mut body = serde_json::to_vec_pretty(&new).map_err(io::Error::other)?;
    body.push(b'\n');
    if old.as_deref() == Some(body.as_slice()) {
        return Ok(());
    }
    if let Some(b) = &old {
        fs::write(config_dir.join("settings.json.bak-office"), b)?;
    }
    put(&path, &body)
}

/// Whether config_dir/settings.json runs office's hooks as this office
/// installs them: an older install's waiter dies after Claude Code's default
/// hook timeout.
pub fn installed(config_dir: &Path) -> bool {
    let Ok(b) = fs::read(config_dir.join("settings.json")) else { return false };
    let Ok(v) = serde_json::from_slice::<Value>(&b) else { return false };
    let ours = |event: &str| v.get("hooks").and_then(|h| h.get(event)).and_then(Value::as_array).into_iter().flatten().filter(|e| is_ours(e)).cloned().collect::<Vec<_>>();
    let waits = |e: &Value| e["hooks"][0]["timeout"].as_u64().is_some_and(|t| t >= WAITER_LIFE.as_secs());
    !ours("PermissionRequest").is_empty() && ["Stop", "SessionStart"].iter().all(|ev| ours(ev).iter().any(waits))
}

/// `office hook install|uninstall`, over every config dir: a report line each.
pub fn manage(install_it: bool, config_dirs: &[PathBuf], out: &mut impl Write) -> io::Result<()> {
    let exe = env::current_exe()?.to_string_lossy().into_owned();
    for d in config_dirs {
        install(d, install_it.then_some(exe.as_str()))?;
        let verb = if install_it { "installed in" } else { "removed from" };
        writeln!(out, "office hooks {verb} {}", d.join("settings.json").display())?;
    }
    Ok(())
}

/// Marks every agent with a permission request or question waiting here
/// as Waiting, its call as what it waits on: exact, where the transcript
/// only lets the scan guess (it cannot see what auto mode asks the user).
pub fn overlay(snap: &mut crate::scan::Snapshot, root: &Path) {
    let mut asks: Vec<(String, String, String)> = Vec::new(); // session, agent ("" for the lead), summary
    for d in fs::read_dir(root).into_iter().flatten().flatten() {
        let session = d.file_name().to_string_lossy().into_owned();
        for e in fs::read_dir(d.path()).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(pid) = name.strip_prefix("ask-").and_then(|n| n.strip_suffix(".json")) else { continue };
            if !hook_alive(pid) {
                continue;
            }
            let Ok(v) = serde_json::from_slice::<Value>(&fs::read(e.path()).unwrap_or_default()) else { continue };
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            asks.push((session.clone(), s("agent"), s("summary")));
        }
    }
    for a in snap.by_id.values_mut() {
        let raw = |id: &str| id.split('@').next().unwrap_or("").to_string();
        let hit = asks.iter().find(|(session, agent, _)| {
            raw(&a.session_id) == *session && if agent.is_empty() { a.kind == crate::scan::Kind::Session } else { raw(&a.id) == *agent }
        });
        if let Some((_, _, summary)) = hit {
            a.status = crate::scan::Status::Waiting;
            if !summary.is_empty() {
                a.last_tool = summary.clone();
            }
        }
    }
}

// Resuming a session that no process holds, in the background.

/// The sessions running Claude Code processes of config_dir hold, by the
/// id in their sessions/<pid>.json, background ones included.
pub fn live_ids(config_dir: &Path) -> std::collections::HashSet<String> {
    fs::read_dir(config_dir.join("sessions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let v = serde_json::from_slice::<Value>(&fs::read(e.path()).ok()?).ok()?;
            v.get("pid").and_then(Value::as_i64).filter(|p| alive_pid(*p))?;
            Some(v.get("sessionId")?.as_str()?.to_string())
        })
        .collect()
}

#[cfg(unix)]
fn alive_pid(pid: i64) -> bool {
    let Ok(pid) = i32::try_from(pid) else { return false };
    // SAFETY: signal 0 only asks whether the process exists.
    pid > 0 && (unsafe { libc::kill(pid, 0) } == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}
#[cfg(not(unix))]
fn alive_pid(_: i64) -> bool {
    false // not known here: resuming then lets Claude Code say it runs already
}

/// The claude program, as office runs it for session: in its folder (gone:
/// an error saying so), under its account's config dir, as if from a plain shell.
fn claude(program: &Path, config_dir: &Path, cwd: &str) -> io::Result<process::Command> {
    if !cwd.is_empty() && !Path::new(cwd).is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("its folder {} no longer exists", tilde(cwd))));
    }
    let mut cmd = process::Command::new(program);
    if !cwd.is_empty() {
        cmd.current_dir(cwd);
    }
    // Run as from a plain shell: a session office was started from must not
    // make this one its child.
    for (k, _) in env::vars_os() {
        if k.to_string_lossy().starts_with("CLAUDE") {
            cmd.env_remove(&k);
        }
    }
    // ~/.claude is the default; naming it would move Claude Code's own config file.
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    if config_dir != home.join(".claude") && !config_dir.as_os_str().is_empty() {
        cmd.env("CLAUDE_CONFIG_DIR", config_dir);
    }
    Ok(cmd)
}

/// Whether the account of config_dir trusts folder itself, as `claude --bg`
/// wants it (a trusted parent folder is not enough there): its
/// projects.<folder>.hasTrustDialogAccepted, in ~/.claude.json for ~/.claude,
/// else in <config_dir>/.claude.json.
pub fn trusts(config_dir: &Path, folder: &str) -> bool {
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let file = if config_dir == home.join(".claude") { home.join(".claude.json") } else { config_dir.join(".claude.json") };
    let Ok(b) = fs::read(file) else { return false };
    let v: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
    v.get("projects").and_then(|p| p.get(folder.trim_end_matches('/'))).and_then(|p| p.get("hasTrustDialogAccepted")).and_then(Value::as_bool) == Some(true)
}

fn not_on_path(e: io::Error) -> io::Error {
    match e.kind() {
        io::ErrorKind::NotFound => io::Error::new(e.kind(), "`claude` is not on PATH"),
        _ => e,
    }
}

/// What office starts `claude` for: a session to resume (its id, as the
/// scan names it), or a new one with its first prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    Resume(String),
    New(String),
}

impl Launch {
    /// claude's arguments for it. A prompt follows `--`, so one such as
    /// `logs` or `-v` is a prompt, not a command or a flag of claude's.
    fn args(&self) -> Vec<&str> {
        match self {
            Launch::Resume(session) => vec!["--resume", session.split('@').next().unwrap_or(session)],
            Launch::New(prompt) => vec!["--", prompt],
        }
    }
}

/// Starts claude for launch in the background (`--bg`) in cwd under
/// config_dir: Claude Code's own background session, needing no terminal
/// (`claude attach` shows it). It returns once the session started; its
/// output says how it went.
pub fn start(program: &Path, config_dir: &Path, cwd: &str, launch: &Launch) -> io::Result<process::Child> {
    let mut cmd = claude(program, config_dir, cwd)?;
    cmd.arg("--bg").args(launch.args()).stdin(process::Stdio::null()).stdout(process::Stdio::piped()).stderr(process::Stdio::piped());
    cmd.spawn().map_err(not_on_path)
}

/// Runs claude for launch in this terminal until it exits: for what a
/// background session cannot do, such as asking whether to trust its
/// folder. The caller hands the terminal over first and takes it back after.
pub fn run_here(program: &Path, config_dir: &Path, cwd: &str, launch: &Launch) -> io::Result<process::ExitStatus> {
    let mut cmd = claude(program, config_dir, cwd)?;
    cmd.args(launch.args()).status().map_err(not_on_path)
}

/// The short id `claude --bg` names the session it started with:
/// "backgrounded · f328bbdc (idle — …)".
pub fn started_id(said: &str) -> Option<&str> {
    let rest = said.split("backgrounded · ").nth(1)?;
    let id = rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').next()?;
    (!id.is_empty()).then_some(id)
}

/// Whether a failed background resume needs a terminal: Claude Code wants
/// the folder trusted first (always so for the home directory).
pub fn needs_terminal(said: &str) -> bool {
    said.contains("Workspace not trusted")
}

/// What to tell the user when `claude --bg` failed for launch, from what it
/// said: its reason, and for a folder this account has not trusted, the
/// command that trusts it (Claude Code's own dialog; office never trusts a
/// folder itself).
pub fn launch_failed(said: &str, config_dir: &Path, cwd: &str, launch: &Launch) -> String {
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let account = if config_dir == home.join(".claude") { String::new() } else { format!("CLAUDE_CONFIG_DIR={} ", tilde(&config_dir.to_string_lossy())) };
    let (not, it, claude) = match launch {
        Launch::Resume(s) => ("not resumed", "it was started in", format!("claude --resume {}", s.split('@').next().unwrap_or(s))),
        Launch::New(_) => ("not started", "it is", "claude".to_string()),
    };
    if said.contains("is your home directory") {
        return format!(
            "{not}: {it} your home directory, and Claude Code never runs those in the background (it never saves trust for home). Press o to run it here (office comes back when you exit Claude), or in a terminal: cd ~ && {account}{claude}"
        );
    }
    if said.contains("Workspace not trusted") {
        return format!(
            "{not}: {} is not trusted for {} yet (a trusted parent folder is not enough in the background). Press o to run it here and accept the trust prompt (office comes back when you exit Claude), or run this once in a terminal: cd {} && {account}claude",
            tilde(cwd),
            tilde(&config_dir.to_string_lossy()),
            tilde(cwd)
        );
    }
    format!("{not}: {said}")
}

/// path with the home directory written as ~.
pub(crate) fn tilde(path: &str) -> String {
    let home = env::var("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("office-bridge-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Prompts sent before the session takes one queue up, each its own:
    /// the waiter takes the oldest, ctrl+x takes back the newest.
    #[test]
    fn prompts_queue_in_order_and_none_replaces_another() {
        let root = tmp("queue");
        for p in ["first", "second", "third"] {
            send(&root, "s1", p).unwrap();
        }
        assert_eq!(queued(&root, "s1"), ["first", "second", "third"]);
        assert_eq!(take(&session_dir(&root, "s1")).as_deref(), Some("first"));
        assert_eq!(cancel(&root, "s1").unwrap().as_deref(), Some("third"));
        assert_eq!(queued(&root, "s1"), ["second"]);
        assert_eq!(take(&session_dir(&root, "s1")).as_deref(), Some("second"));
        assert_eq!((take(&session_dir(&root, "s1")), cancel(&root, "s1").unwrap()), (None, None));
        assert!(queued(&root, "s1").is_empty());
    }

    /// Installing keeps every other setting and hook, twice is once, and
    /// uninstalling gives back what was there.
    #[test]
    fn install_adds_and_removes_only_its_own_hooks() {
        let d = tmp("install");
        let theirs = json!({ "model": "opus", "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "notify-send done" }] }] }, "z": 1 });
        fs::write(d.join("settings.json"), serde_json::to_vec_pretty(&theirs).unwrap()).unwrap();
        assert!(!installed(&d));
        install(&d, Some("/opt/office")).unwrap();
        install(&d, Some("/opt/office")).unwrap();
        assert!(installed(&d));
        let v: Value = serde_json::from_slice(&fs::read(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2, "theirs and ours, once: {v}");
        assert_eq!(v["hooks"]["Stop"][1]["hooks"][0]["asyncRewake"], true);
        for event in ["Stop", "SessionStart"] {
            let ours = v["hooks"][event].as_array().unwrap().last().unwrap();
            assert_eq!(ours["hooks"][0]["timeout"], WAITER_LIFE.as_secs(), "Claude Code would kill the waiter after its default 600 s: {event}");
        }
        let mut old = v.clone();
        old["hooks"]["Stop"][1]["hooks"][0].as_object_mut().unwrap().remove("timeout");
        fs::write(d.join("settings.json"), serde_json::to_vec_pretty(&old).unwrap()).unwrap();
        assert!(!installed(&d), "an install from before the waiter's timeout is out of date");
        install(&d, Some("/opt/office")).unwrap();
        assert!(installed(&d), "installing again brings it up to date");
        assert_eq!(v["hooks"]["PermissionRequest"][0]["hooks"][0]["command"], "\"/opt/office\" hook permission");
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "hooks", "z"], "the user's key order is kept");
        install(&d, None).unwrap();
        let back: Value = serde_json::from_slice(&fs::read(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(back, theirs);
        assert!(d.join("settings.json.bak-office").exists());
        let _ = fs::remove_dir_all(&d);
    }

    /// office's answers become the hook's decisions; "don't ask again"
    /// hands back Claude Code's own suggestion.
    #[test]
    fn answers_become_decisions() {
        let sugg = json!([{ "type": "addRules", "rules": [{ "toolName": "Bash", "ruleContent": "ls:*" }], "behavior": "allow", "destination": "localSettings" }]);
        let v = json!({ "permission_suggestions": sugg, "tool_input": { "questions": [{ "question": "Color?" }] } });
        let get = |a: &str| decision(a, &v).map(|s| serde_json::from_str::<Value>(&s).unwrap()["hookSpecificOutput"]["decision"].clone());
        assert_eq!(get("allow"), Some(json!({ "behavior": "allow" })));
        assert_eq!(get("always"), Some(json!({ "behavior": "allow", "updatedPermissions": sugg })));
        assert_eq!(get("deny").unwrap()["behavior"], "deny");
        assert_eq!(get("terminal"), None);
        let picked = Answer::Answers(vec![("Color?".into(), "Blue".into())]).body();
        assert_eq!(get(&picked), Some(json!({ "behavior": "allow", "updatedInput": { "questions": [{ "question": "Color?" }], "answers": { "Color?": "Blue" } } })));
    }

    /// The whole round trip with a real hook process: office alive, the hook
    /// files its request, office sees and answers it, the hook prints the decision.
    #[test]
    fn a_permission_request_waits_for_office() {
        let root = tmp("perm");
        heartbeat(&root);
        let v = json!({ "session_id": "s1", "tool_name": "Bash", "tool_input": { "command": "rm -rf build" } });
        let r = root.clone();
        let hook = thread::spawn(move || permission(&r.join("s1"), &r, &v));
        let a = (0..100).find_map(|_| {
            thread::sleep(POLL);
            heartbeat(&root);
            ask(&root, "s1@work")
        });
        let a = a.expect("the request shows up for office");
        assert_eq!(a.summary, "Bash: rm -rf build");
        answer(&root, "s1", &a, Answer::Deny).unwrap();
        let out: Value = serde_json::from_str(&hook.join().unwrap().expect("a decision")).unwrap();
        assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert_eq!(ask(&root, "s1"), None, "the hook cleans up");
        leave(&root);
        let v = json!({ "session_id": "s1", "tool_name": "Bash", "tool_input": {} });
        assert_eq!(permission(&root.join("s1"), &root, &v), None, "office gone: left to Claude Code's dialog at once");
        let _ = fs::remove_dir_all(&root);
    }

    /// Answered in Claude Code's own dialog: the call's result reaches the
    /// asking agent's transcript (a subagent's, here), and the hook lets go.
    #[test]
    fn a_request_answered_in_the_terminal_is_dropped() {
        let root = tmp("terminal");
        heartbeat(&root);
        let main = root.join("proj").join("s1.jsonl");
        let sub = root.join("proj").join("s1").join("subagents").join("agent-a7.jsonl");
        fs::create_dir_all(sub.parent().unwrap()).unwrap();
        fs::write(&sub, "{\"type\":\"assistant\"}\n").unwrap();
        let v = json!({ "session_id": "s1", "transcript_path": main, "agent_id": "a7", "tool_name": "Bash", "tool_input": {} });
        let r = root.clone();
        let hook = thread::spawn(move || permission(&r.join("s1"), &r, &v));
        while ask(&root, "s1").is_none() {
            thread::sleep(POLL);
            heartbeat(&root);
        }
        let mut f = fs::OpenOptions::new().append(true).open(&sub).unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t"}}]}}}}"#).unwrap();
        assert_eq!(hook.join().unwrap(), None);
        assert_eq!(ask(&root, "s1"), None, "nothing left for the panel");
        let _ = fs::remove_dir_all(&root);
    }

    /// A waiter takes the prompt office leaves; a newer waiter retires an older one.
    #[test]
    fn a_waiter_takes_the_prompt() {
        let root = tmp("wait");
        let d = root.join("s1");
        let first = {
            let d = d.clone();
            thread::spawn(move || wait(&d))
        };
        while !d.join("waiter").exists() {
            thread::sleep(POLL);
        }
        put(&d.join("waiter"), b"someone-newer").unwrap();
        assert_eq!(first.join().unwrap(), None, "retired");
        let second = {
            let d = d.clone();
            thread::spawn(move || wait(&d))
        };
        while fs::read_to_string(d.join("waiter")).ok().as_deref() != Some(process::id().to_string().as_str()) {
            thread::sleep(POLL);
        }
        assert!(send(&root, "s1@work", "run the tests").unwrap(), "a waiter is there");
        assert_eq!(second.join().unwrap().as_deref(), Some("run the tests"));
        assert!(queued(&root, "s1").is_empty() && !d.join("waiter").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// A session is live while the process its sessions/<pid>.json names runs.
    #[test]
    fn live_follows_the_sessions_registry() {
        let d = tmp("live");
        fs::create_dir_all(d.join("sessions")).unwrap();
        let me = process::id();
        fs::write(d.join("sessions").join(format!("{me}.json")), json!({ "pid": me, "sessionId": "s1" }).to_string()).unwrap();
        fs::write(d.join("sessions").join("1.json"), json!({ "pid": 999_999_999i64, "sessionId": "s2" }).to_string()).unwrap();
        let held = live_ids(&d);
        assert!(held.contains("s1"));
        assert!(!held.contains("s2"), "its process is gone");
        assert!(!held.contains("s3"), "never registered");
        let _ = fs::remove_dir_all(&d);
    }

    /// `claude -p` and SDK runs get no waiter and no held request; terminal
    /// sessions, background ones and unknown entrypoints do.
    #[test]
    fn headless_runs_are_left_alone() {
        for e in ["sdk-cli", "sdk-ts", "sdk-py"] {
            assert!(headless(Some(e)), "{e}");
        }
        for e in [Some("cli"), Some("claude-vscode"), None] {
            assert!(!headless(e), "{e:?}");
        }
    }

    /// A request filed by a hook marks its agent Waiting, the lead's or a
    /// subagent's, whatever the transcript says; a dead hook's file does not.
    #[test]
    fn requests_mark_their_agent_waiting() {
        use crate::scan::{Agent, Kind, Snapshot, Status};
        let root = tmp("overlay");
        let me = process::id();
        let agent = |id: &str, kind: Kind| Agent { id: id.into(), session_id: "s1@work".into(), kind, status: Status::Running, ..Agent::default() };
        let mut snap = Snapshot::default();
        for a in [agent("s1@work", Kind::Session), agent("a7", Kind::Subagent), agent("a8", Kind::Subagent)] {
            snap.by_id.insert(a.id.clone(), a);
        }
        put(&root.join("s1").join(format!("ask-{me}.json")), json!({ "summary": "Bash: make", "agent": "a7" }).to_string().as_bytes()).unwrap();
        put(&root.join("s1").join("ask-999999999.json"), json!({ "summary": "Bash: gone", "agent": "" }).to_string().as_bytes()).unwrap();
        overlay(&mut snap, &root);
        assert_eq!((snap.by_id["a7"].status, snap.by_id["a7"].last_tool.as_str()), (Status::Waiting, "Bash: make"));
        assert_eq!(snap.by_id["a8"].status, Status::Running);
        assert_eq!(snap.by_id["s1@work"].status, Status::Running, "its hook is gone");
        assert_eq!(ask(&root, "s1").map(|a| a.summary), Some("Bash: make".into()), "the panel asks only what a live hook waits on");
        fs::remove_file(root.join("s1").join(format!("ask-{me}.json"))).unwrap();
        assert_eq!(ask(&root, "s1"), None, "a dead hook's question is not asked");
        let _ = fs::remove_dir_all(&root);
    }

    /// A failed resume says why and how to fix it: the trust command for this
    /// account's config dir, the home-directory case, a vanished folder.
    #[test]
    fn a_failed_resume_says_why_and_what_to_run() {
        let home = PathBuf::from(env::var("HOME").unwrap());
        let (work, dflt) = (home.join(".claude-work"), home.join(".claude"));
        let cwd = home.join("dev").join("api").to_string_lossy().into_owned();
        let said = format!("Workspace not trusted. Run `claude` in {cwd} once and accept the trust prompt, then retry.");
        let (s1, s1_work, new) = (Launch::Resume("s1".into()), Launch::Resume("s1@work".into()), Launch::New("hi".into()));
        let m = launch_failed(&said, &work, &cwd, &s1_work);
        assert!(m.contains("~/dev/api is not trusted for ~/.claude-work yet") && m.contains("Press o to run it here") && m.ends_with("cd ~/dev/api && CLAUDE_CONFIG_DIR=~/.claude-work claude"), "{m}");
        assert!(needs_terminal(&said) && !needs_terminal("boom"));
        assert!(launch_failed(&said, &dflt, &cwd, &s1).ends_with("cd ~/dev/api && claude"), "the default account needs no CLAUDE_CONFIG_DIR");
        assert!(launch_failed(&said, &work, &cwd, &new).starts_with("not started: ~/dev/api is not trusted"));
        let at_home = format!("Error: Workspace not trusted. {} is your home directory, and ...", home.display());
        let m = launch_failed(&at_home, &work, &home.to_string_lossy(), &s1_work);
        assert!(m.contains("home directory") && m.ends_with("cd ~ && CLAUDE_CONFIG_DIR=~/.claude-work claude --resume s1"), "{m}");
        let m = launch_failed(&at_home, &work, &home.to_string_lossy(), &new);
        assert!(m.starts_with("not started: it is your home directory") && m.ends_with("cd ~ && CLAUDE_CONFIG_DIR=~/.claude-work claude"), "{m}");
        assert_eq!(launch_failed("boom", &work, &cwd, &s1), "not resumed: boom");
        let gone = start(Path::new("claude"), &work, "/nonexistent/office-gone", &s1).unwrap_err();
        assert_eq!(gone.to_string(), "its folder /nonexistent/office-gone no longer exists");
    }

    /// A request shows its whole call, an edit as a diff, and what "don't
    /// ask again" saves, where.
    #[test]
    fn a_request_shows_its_whole_call_and_what_always_saves() {
        assert_eq!(detail("Bash", &json!({ "command": "cargo build &&\n  cargo test" })), ["cargo build &&", "  cargo test"]);
        let edit = json!({ "file_path": "/p/a.rs", "old_string": "x\ny", "new_string": "z" });
        assert_eq!(detail("Edit", &edit), ["/p/a.rs", "- x", "- y", "+ z"]);
        assert_eq!(detail("WebFetch", &json!({ "url": "https://x", "n": 2 })), ["url: https://x", "n: 2"], "in the call's own order");
        let s = json!([
            { "type": "addRules", "rules": [{ "toolName": "Bash", "ruleContent": "npm test:*" }, { "toolName": "Read" }], "behavior": "allow", "destination": "projectSettings" },
            { "type": "addDirectories", "directories": ["/tmp/x"], "destination": "session" },
            { "type": "setMode", "mode": "acceptEdits", "destination": "session" },
        ]);
        assert_eq!(rules(&s), ["Bash(npm test:*) (this project)", "Read (this project)", "folder /tmp/x (this session)", "mode acceptEdits (this session)"]);
        assert!(rules(&Value::Null).is_empty());
    }

    /// An account trusts the folders its .claude.json accepted the trust
    /// dialog for, and only those: not their subfolders.
    #[test]
    fn an_account_trusts_only_the_folders_it_accepted() {
        let cfg = tmp("trust");
        fs::create_dir_all(&cfg).unwrap();
        let v = json!({ "projects": { "/p": { "hasTrustDialogAccepted": true }, "/q": { "hasTrustDialogAccepted": false } } });
        fs::write(cfg.join(".claude.json"), v.to_string()).unwrap();
        assert!(trusts(&cfg, "/p") && trusts(&cfg, "/p/"));
        assert!(!trusts(&cfg, "/p/sub") && !trusts(&cfg, "/q") && !trusts(&cfg, "/r"));
        assert!(!trusts(&cfg.join("none"), "/p"), "no such account");
        let _ = fs::remove_dir_all(&cfg);
    }

    /// A new session's prompt goes after `--`, so claude never takes it for
    /// a command or a flag; `--bg` names what it started.
    #[test]
    fn a_new_session_starts_with_its_prompt() {
        assert_eq!(Launch::New("logs".into()).args(), ["--", "logs"]);
        assert_eq!(Launch::Resume("s1@work".into()).args(), ["--resume", "s1"]);
        assert_eq!(started_id("backgrounded · f328bbdc (idle — send a prompt to start)\n  claude agents"), Some("f328bbdc"));
        assert_eq!(started_id("Workspace not trusted."), None);
    }
}
