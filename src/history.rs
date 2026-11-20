//! The prompts the prompt panel recalls: Claude Code's own history of a project, and the prompts office sent there.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

/// The prompts typed for project, oldest first: Claude Code's history in
/// config_dir, its pasted texts filled in, merged by time with office's in
/// bridge; a prompt repeating the one before it is left out.
pub fn load(config_dir: &Path, bridge: &Path, project: &str) -> Vec<String> {
    let mut all: Vec<(i64, String)> = Vec::new();
    for file in [config_dir.join("history.jsonl"), bridge.join("history.jsonl")] {
        let Ok(text) = fs::read_to_string(&file) else { continue };
        for v in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            if v["project"].as_str() != Some(project) {
                continue;
            }
            let t = pasted(v["display"].as_str().unwrap_or(""), &v["pastedContents"]);
            if !t.trim().is_empty() {
                all.push((v["timestamp"].as_i64().unwrap_or(0), t));
            }
        }
    }
    all.sort_by_key(|(at, _)| *at);
    let mut out: Vec<String> = Vec::new();
    for (_, t) in all {
        if out.last() != Some(&t) {
            out.push(t);
        }
    }
    out
}

/// Keeps prompt, sent from office, for project's history. Only office writes
/// this file: Claude Code's own is Claude Code's to write.
pub fn add(bridge: &Path, project: &str, prompt: &str) -> io::Result<()> {
    fs::create_dir_all(bridge)?;
    let at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
    let line = format!("{}\n", json!({ "display": prompt, "timestamp": at, "project": project }));
    OpenOptions::new().create(true).append(true).open(bridge.join("history.jsonl"))?.write_all(line.as_bytes())
}

/// display with each `[Pasted text #N…]` replaced by the text it stands for.
fn pasted(display: &str, contents: &Value) -> String {
    let mut out = display.to_string();
    for (id, p) in contents.as_object().into_iter().flatten() {
        let Some(text) = p["content"].as_str().filter(|_| p["type"] == "text") else { continue };
        let mark = format!("[Pasted text #{id}");
        if let Some(i) = out.find(&mark)
            && let Some(j) = out[i..].find(']')
        {
            out.replace_range(i..i + j + 1, text);
        }
    }
    out
}
