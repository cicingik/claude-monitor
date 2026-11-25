//! The bridge between office and running Claude Code sessions, through
//! Claude Code's own hooks, so it works in any terminal (or none).
//!
//! office installs one hook (`office hook install`), this binary:
//! PermissionRequest runs `office hook permission`. While office runs, it
//! files the request as ask-<pid>.json, which makes the asking agent's
//! Waiting exact (see overlay), until Claude Code's own dialog is answered:
//! the call's result reaches the transcript, or HOLD passes. When office is
//! not running, the hook returns at once. It never answers the request.
//!
//! Files live in $XDG_STATE_HOME/claude-monitor/bridge (else
//! ~/.local/state/claude-monitor/bridge), one directory per session id, each
//! file written whole (to a temporary name, then renamed).
//! `alive` is office's heartbeat: the hook treats office as gone once it is
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

/// The longest a permission request stays filed. Claude Code's own dialog
/// is up all along, so the hook costs the session nothing.
const HOLD: Duration = Duration::from_secs(600);
/// office counts as gone once its heartbeat is this old.
const STALE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(150);
/// What every command office installs starts with, after the binary's path.
const MARK: &str = " hook ";

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

// The hook's side: `office hook permission`, with the hook's JSON on stdin.

/// Whether the session runs headless (`claude -p`, the Agent SDK), going by
/// the CLAUDE_CODE_ENTRYPOINT its hooks see: "sdk-cli", "sdk-ts", ... Such a
/// run ends on its own, and a held request would stall it, so the hook does
/// nothing there.
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
            permission(&d, root, &v);
            0
        }
        // `wait`, the prompt waiter older installs run, does nothing now.
        _ => 0,
    }
}

fn alive(root: &Path) -> bool {
    age(&root.join("alive")).is_some_and(|a| a < STALE)
}

/// Files the request until Claude Code's dialog is answered, office is
/// gone, or HOLD passes.
fn permission(d: &Path, root: &Path, v: &Value) {
    if !alive(root) {
        return;
    }
    let tool = v.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let summary = crate::scan::tool_summary(tool, v.get("tool_input").unwrap_or(&Value::Null), 200);
    let ask = d.join(format!("ask-{}.json", process::id()));
    let agent = v.get("agent_id").and_then(Value::as_str).unwrap_or("");
    if put(&ask, json!({ "summary": summary, "agent": agent }).to_string().as_bytes()).is_err() {
        return;
    }
    // Claude Code leaves this hook running when its dialog is answered: the
    // call's tool_result in the transcript of the agent that asked says so.
    let mut watch = transcript(v).map(|p| (fs::metadata(&p).map_or(0, |m| m.len()), p));
    let start = SystemTime::now();
    loop {
        if !alive(root) || start.elapsed().unwrap_or_default() >= HOLD {
            break;
        }
        if let Some((from, p)) = &mut watch
            && let Ok(b) = fs::read(&*p)
            && b.len() as u64 > *from
        {
            if b[*from as usize..].windows(13).any(|w| w == b"\"tool_result\"") {
                break;
            }
            *from = b.len() as u64;
        }
        thread::sleep(POLL);
    }
    let _ = fs::remove_file(&ask);
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

// Installing the hooks into a Claude Code config directory's settings.json.

/// The hook entries office adds, run by exe.
fn entries(exe: &str) -> [(&'static str, Value); 1] {
    let cmd = |kind: &str| format!("\"{exe}\"{MARK}{kind}");
    let permission = json!({ "matcher": "*", "hooks": [{ "type": "command", "command": cmd("permission"), "timeout": HOLD.as_secs() + 30 }] });
    [("PermissionRequest", permission)]
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

/// Which Claude Code process holds a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    None,
    Background { job: String }, // `claude --bg`'s: `claude attach <job>` shows it
    Terminal,                   // a claude open in a terminal of its own
}

/// Who holds session (its id, as the scan names it) of config_dir, from
/// the sessions/<pid>.json of running Claude Code processes.
pub fn holder(config_dir: &Path, session: &str) -> Holder {
    let raw = session.split('@').next().unwrap_or(session);
    for e in fs::read_dir(config_dir.join("sessions")).into_iter().flatten().flatten() {
        let Ok(v) = serde_json::from_slice::<Value>(&fs::read(e.path()).unwrap_or_default()) else { continue };
        let s = |k: &str| v.get(k).and_then(Value::as_str);
        // Off unix a pid is not checked: a stale file's attach says so itself.
        if s("sessionId") != Some(raw) || !v.get("pid").and_then(Value::as_i64).is_some_and(|p| !cfg!(unix) || alive_pid(p)) {
            continue;
        }
        return match (s("kind"), s("jobId")) {
            (Some("bg"), Some(job)) => Holder::Background { job: job.into() },
            _ => Holder::Terminal,
        };
    }
    Holder::None
}

/// `claude attach <job>` in session's folder under config_dir, for the panel's terminal.
pub fn attach(program: &Path, config_dir: &Path, cwd: &str, job: &str) -> io::Result<process::Command> {
    let mut cmd = claude(program, config_dir, cwd)?;
    cmd.args(["attach", job]);
    Ok(cmd)
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
            "{not}: {it} your home directory, and Claude Code never runs those in the background (it never saves trust for home). Press p to run it here (office comes back when you exit Claude), or in a terminal: cd ~ && {account}{claude}"
        );
    }
    if said.contains("Workspace not trusted") {
        return format!(
            "{not}: {} is not trusted for {} yet (a trusted parent folder is not enough in the background). Press p to run it here and accept the trust prompt (office comes back when you exit Claude), or run this once in a terminal: cd {} && {account}claude",
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

    /// The summary of the request the hook filed for session, if one is.
    fn filed(root: &Path, session: &str) -> Option<String> {
        let d = fs::read_dir(root.join(session)).ok()?;
        d.flatten().find(|e| e.file_name().to_string_lossy().starts_with("ask-")).and_then(|e| {
            let v: Value = serde_json::from_slice(&fs::read(e.path()).ok()?).ok()?;
            Some(v.get("summary")?.as_str()?.to_string())
        })
    }

    fn tmp(name: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("office-bridge-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A background session is attached by its job id; one open in a
    /// terminal, or held by a process gone, is not.
    #[test]
    fn holder_tells_background_sessions_from_terminal_ones() {
        let d = tmp("holder");
        fs::create_dir_all(d.join("sessions")).unwrap();
        let me = process::id();
        let put = |n: &str, v: Value| fs::write(d.join("sessions").join(n), v.to_string()).unwrap();
        put("1.json", json!({"pid": me, "sessionId": "bg-1", "kind": "bg", "jobId": "bg"}));
        put("2.json", json!({"pid": me, "sessionId": "tty-1", "kind": "interactive"}));
        put("3.json", json!({"pid": i32::MAX, "sessionId": "gone-1", "kind": "bg", "jobId": "gone"}));
        assert_eq!(holder(&d, "bg-1@work"), Holder::Background { job: "bg".into() });
        assert_eq!(holder(&d, "tty-1"), Holder::Terminal);
        assert_eq!(holder(&d, "gone-1"), Holder::None);
        assert_eq!(holder(&d, "never"), Holder::None);
    }

    /// Installing keeps every other setting and hook, twice is once, and
    /// uninstalling gives back what was there.
    #[test]
    fn install_adds_and_removes_only_its_own_hooks() {
        let d = tmp("install");
        let theirs = json!({ "model": "opus", "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "notify-send done" }] }] }, "z": 1 });
        fs::write(d.join("settings.json"), serde_json::to_vec_pretty(&theirs).unwrap()).unwrap();
        // An older install's prompt waiter, which installing again removes.
        let mut old = theirs.clone();
        old["hooks"]["Stop"].as_array_mut().unwrap().push(json!({ "hooks": [{ "type": "command", "command": "\"/opt/office\" hook wait", "asyncRewake": true }] }));
        old["hooks"]["SessionStart"] = json!([{ "hooks": [{ "type": "command", "command": "\"/opt/office\" hook wait" }] }]);
        fs::write(d.join("settings.json"), serde_json::to_vec_pretty(&old).unwrap()).unwrap();
        install(&d, Some("/opt/office")).unwrap();
        install(&d, Some("/opt/office")).unwrap();
        let v: Value = serde_json::from_slice(&fs::read(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["hooks"]["Stop"], theirs["hooks"]["Stop"], "theirs only, the old waiter gone: {v}");
        assert!(v["hooks"].get("SessionStart").is_none(), "{v}");
        assert_eq!(v["hooks"]["PermissionRequest"].as_array().unwrap().len(), 1, "ours, once: {v}");
        assert_eq!(v["hooks"]["PermissionRequest"][0]["hooks"][0]["command"], "\"/opt/office\" hook permission");
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "hooks", "z"], "the user's key order is kept");
        install(&d, None).unwrap();
        let back: Value = serde_json::from_slice(&fs::read(d.join("settings.json")).unwrap()).unwrap();
        assert_eq!(back, theirs);
        assert!(d.join("settings.json.bak-office").exists());
        let _ = fs::remove_dir_all(&d);
    }

    /// While office runs, the hook files the request for the tree's Waiting;
    /// once office is gone it cleans up and lets go.
    #[test]
    fn a_permission_request_is_filed_while_office_runs() {
        let root = tmp("perm");
        heartbeat(&root);
        let v = json!({ "session_id": "s1", "tool_name": "Bash", "tool_input": { "command": "rm -rf build" } });
        let r = root.clone();
        let hook = thread::spawn(move || permission(&r.join("s1"), &r, &v));
        let summary = (0..100).find_map(|_| {
            thread::sleep(POLL);
            heartbeat(&root);
            filed(&root, "s1")
        });
        assert_eq!(summary.as_deref(), Some("Bash: rm -rf build"), "the request shows up for office");
        leave(&root);
        hook.join().unwrap();
        assert_eq!(filed(&root, "s1"), None, "office gone: the hook cleans up and returns");
        let v = json!({ "session_id": "s1", "tool_name": "Bash", "tool_input": {} });
        permission(&root.join("s1"), &root, &v);
        assert_eq!(filed(&root, "s1"), None, "office not running: nothing is filed");
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
        while filed(&root, "s1").is_none() {
            thread::sleep(POLL);
            heartbeat(&root);
        }
        let mut f = fs::OpenOptions::new().append(true).open(&sub).unwrap();
        writeln!(f, r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t"}}]}}}}"#).unwrap();
        hook.join().unwrap();
        assert_eq!(filed(&root, "s1"), None, "answered in its dialog: the request is gone");
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

    /// `claude -p` and SDK runs get no held request; terminal
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
        assert!(m.contains("~/dev/api is not trusted for ~/.claude-work yet") && m.contains("Press p to run it here") && m.ends_with("cd ~/dev/api && CLAUDE_CONFIG_DIR=~/.claude-work claude"), "{m}");
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
