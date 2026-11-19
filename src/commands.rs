//! The slash commands a session knows, for the prompt panel: its skills and
//! custom commands, from the session's account, its project and its enabled
//! plugins, and Claude Code's built-ins.
//!
//! office sends a prompt as hook feedback, which Claude Code never parses as a
//! command, so office turns a command into what the session reads instead:
//! the file that defines it, to follow. A built-in has no file, so office
//! keeps it and says so.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub name: String, // without the slash: "commit", "pstack:poteto-mode"
    pub about: String,
    pub file: Option<PathBuf>, // its SKILL.md or command file; None: built into Claude Code
}

/// Claude Code's own commands: only its terminal runs them.
const BUILTINS: &[(&str, &str)] = &[
    ("clear", "clear the conversation"),
    ("compact", "compact the conversation"),
    ("context", "show context usage"),
    ("config", "open settings"),
    ("cost", "show the session's cost"),
    ("exit", "exit Claude Code"),
    ("help", "show help"),
    ("hooks", "manage hooks"),
    ("login", "sign in"),
    ("logout", "sign out"),
    ("mcp", "manage MCP servers"),
    ("memory", "edit memory files"),
    ("model", "pick the model"),
    ("permissions", "manage permissions"),
    ("plugin", "manage plugins"),
    ("resume", "resume a conversation"),
    ("rewind", "rewind the conversation"),
    ("status", "show status"),
];

/// The commands of a session of config_dir in cwd, sorted by name. A name
/// defined twice keeps the first of: project, account, plugin, built-in.
pub fn list(config_dir: &Path, cwd: &Path) -> Vec<Command> {
    let mut found: Vec<Command> = Vec::new();
    for base in [cwd.join(".claude"), config_dir.to_path_buf()] {
        found.extend(skills(&base.join("skills"), ""));
        found.extend(commands(&base.join("commands"), ""));
    }
    if let Ok(buckets) = fs::read_dir(config_dir.join("skills").join("synced")) {
        for b in buckets.flatten() {
            found.extend(skills(&b.path(), "anthropic-skills:"));
        }
    }
    for (plugin, dir) in plugins(config_dir) {
        let prefix = format!("{plugin}:");
        found.extend(skills(&dir.join("skills"), &prefix));
        found.extend(commands(&dir.join("commands"), &prefix));
    }
    found.extend(BUILTINS.iter().map(|(n, a)| Command { name: (*n).into(), about: (*a).into(), file: None }));
    let mut by_name = BTreeMap::new();
    for c in found {
        by_name.entry(c.name.clone()).or_insert(c);
    }
    by_name.into_values().collect()
}

/// The commands for what is typed: while it is a slash and a name, no
/// space yet, those whose name, or its part after the plugin, starts with it.
pub fn matching<'a>(all: &'a [Command], typed: &str) -> Vec<&'a Command> {
    let Some(word) = typed.strip_prefix('/').filter(|w| !w.contains(char::is_whitespace)) else { return Vec::new() };
    let after = |c: &Command| c.name.split_once(':').is_some_and(|(_, n)| n.starts_with(word));
    let mut out: Vec<&Command> = all.iter().filter(|c| c.name.starts_with(word)).collect();
    out.extend(all.iter().filter(|c| !c.name.starts_with(word) && after(c)));
    out
}

/// What the session reads for a prompt, or why office keeps it.
pub fn message(all: &[Command], prompt: &str) -> Result<String, String> {
    let p = prompt.trim();
    let named = p
        .strip_prefix('/')
        .map(|c| c.split_once(char::is_whitespace).unwrap_or((c, "")))
        .and_then(|(n, args)| Some((all.iter().find(|c| c.name == n)?, args.trim())));
    match named {
        Some((Command { name, file: None, .. }, _)) => Err(format!("/{name} is built into Claude Code: type it in the session's terminal")),
        Some((Command { name, file: Some(f), .. }, args)) => {
            let dir = f.parent().map_or(String::new(), |d| d.display().to_string());
            let args = if args.is_empty() { "none".to_string() } else { format!("\"{args}\"") };
            Ok(format!(
                "The user ran the slash command `{p}` from claude-monitor; treat it as their next message, typed as a command. \
                 Read {} in full and follow it as the command /{name}, with arguments {args}. Paths in it are relative to {dir}.",
                f.display()
            ))
        }
        None => Ok(format!("The user sent this prompt from claude-monitor; treat it as their next message:\n\n{p}")),
    }
}

/// The skills under dir, one folder each with a SKILL.md, named by its
/// frontmatter, else by the folder.
fn skills(dir: &Path, prefix: &str) -> Vec<Command> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let file = e.path().join("SKILL.md");
        let Ok(text) = fs::read_to_string(&file) else { continue };
        let (name, about) = front(&text);
        let name = name.unwrap_or_else(|| e.file_name().to_string_lossy().into_owned());
        out.push(Command { name: format!("{prefix}{name}"), about, file: Some(file) });
    }
    out
}

/// The custom commands under dir: each .md file, a subfolder adding `folder:`.
fn commands(dir: &Path, prefix: &str) -> Vec<Command> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if path.is_dir() {
            out.extend(commands(&path, &format!("{prefix}{stem}:")));
        } else if path.extension().is_some_and(|x| x == "md") {
            let about = fs::read_to_string(&path).map(|t| front(&t).1).unwrap_or_default();
            out.push(Command { name: format!("{prefix}{stem}"), about, file: Some(path) });
        }
    }
    out
}

/// The account's enabled plugins: each one's name and install folder.
fn plugins(config_dir: &Path) -> Vec<(String, PathBuf)> {
    let read = |p: PathBuf| fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
    let settings = read(config_dir.join("settings.json"));
    let installed = read(config_dir.join("plugins").join("installed_plugins.json"));
    let Some(enabled) = settings.get("enabledPlugins").and_then(Value::as_object) else { return Vec::new() };
    enabled
        .iter()
        .filter(|(_, on)| on.as_bool() == Some(true))
        .filter_map(|(key, _)| {
            let path = installed.get("plugins")?.get(key)?.as_array()?.first()?.get("installPath")?.as_str()?;
            Some((key.split('@').next().unwrap_or(key).to_string(), PathBuf::from(path)))
        })
        .collect()
}

/// The name and description in a markdown file's frontmatter.
fn front(text: &str) -> (Option<String>, String) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, String::new());
    }
    let (mut name, mut about) = (None, String::new());
    let unquote = |v: &str| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    for l in lines.take_while(|l| l.trim() != "---") {
        if let Some(v) = l.strip_prefix("name:") {
            name = Some(unquote(v)).filter(|n| !n.is_empty());
        } else if let Some(v) = l.strip_prefix("description:") {
            about = unquote(v);
        }
    }
    (name, about)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(p: &Path, text: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    /// A fake account and project: skills, a command in a subfolder, a synced
    /// skill, an enabled and a disabled plugin, and a project skill shadowing the account's.
    fn setup(tag: &str) -> (PathBuf, Vec<Command>) {
        let root = std::env::temp_dir().join(format!("office-commands-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (cfg, cwd, plug) = (root.join("cfg"), root.join("proj"), root.join("plug"));
        put(&cfg.join("skills/cing/SKILL.md"), "---\nname: cing\ndescription: \"Operating mode\"\n---\nbody");
        put(&cfg.join("skills/review/SKILL.md"), "---\ndescription: account review\n---\n");
        put(&cfg.join("commands/git/commit.md"), "---\ndescription: commit it\n---\n");
        put(&cfg.join("skills/synced/b1/docx/SKILL.md"), "---\nname: docx\n---\n");
        put(&cwd.join(".claude/skills/review/SKILL.md"), "---\ndescription: project review\n---\n");
        put(&plug.join("skills/poteto-mode/SKILL.md"), "---\nname: poteto-mode\ndescription: poteto\n---\n");
        put(&cfg.join("settings.json"), r#"{"enabledPlugins":{"pstack@market":true,"off@market":false}}"#);
        let installed = serde_json::json!({"plugins":{"pstack@market":[{"installPath":plug}],"off@market":[{"installPath":root.join("off")}]}});
        put(&cfg.join("plugins/installed_plugins.json"), &installed.to_string());
        put(&root.join("off/skills/hidden/SKILL.md"), "---\nname: hidden\n---\n");
        let all = list(&cfg, &cwd);
        (root, all)
    }

    #[test]
    fn lists_the_sessions_skills_commands_plugins_and_builtins() {
        let (root, all) = setup("list");
        let names: Vec<&str> = all.iter().map(|c| c.name.as_str()).collect();
        for n in ["cing", "review", "git:commit", "anthropic-skills:docx", "pstack:poteto-mode", "compact"] {
            assert!(names.contains(&n), "{n} in {names:?}");
        }
        assert!(!names.iter().any(|n| n.contains("hidden")), "a disabled plugin's skill: {names:?}");
        assert_eq!(all.iter().find(|c| c.name == "review").unwrap().about, "project review");
        assert_eq!(all.iter().find(|c| c.name == "cing").unwrap().about, "Operating mode");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn matches_a_typed_name_or_its_part_after_the_plugin() {
        let (root, all) = setup("match");
        let names = |t| matching(&all, t).iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("/pot"), ["pstack:poteto-mode"]);
        assert_eq!(names("/pstack:p"), ["pstack:poteto-mode"]);
        assert_eq!(names("/co")[..2], ["compact".to_string(), "config".to_string()]);
        assert!(names("/cing now").is_empty() && names("hi").is_empty());
        assert_eq!(names("/").len(), all.len());
        let _ = fs::remove_dir_all(&root);
    }

    /// A command becomes its file to follow, a built-in stays in office, and
    /// anything else, an unknown name or a path, goes as the prompt.
    #[test]
    fn a_command_is_sent_as_its_file_and_a_builtin_is_kept() {
        let (root, all) = setup("message");
        let m = message(&all, "/pstack:poteto-mode fix the bug\n").unwrap();
        assert!(m.contains("SKILL.md in full") && m.contains("/pstack:poteto-mode, with arguments \"fix the bug\""), "{m}");
        assert!(message(&all, "/cing").unwrap().contains("with arguments none"));
        assert!(message(&all, "/compact").unwrap_err().contains("type it in the session's terminal"));
        for p in ["/architec add autocomplete", "/tmp/x is full", "fix the build"] {
            assert!(message(&all, p).unwrap().ends_with(&format!("\n\n{p}")), "{p}");
        }
        let _ = fs::remove_dir_all(&root);
    }
}
