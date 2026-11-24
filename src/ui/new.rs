//! The new-session picker: n in the tree opens it over the tree pane. It
//! starts `claude --bg` with a first prompt (a session writes nothing, so
//! shows nowhere, before its first prompt) in a folder picked from those the
//! sessions ran in, or typed with tab completing it, under a picked account.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    bridge::{self, tilde},
    edit::LineEdit,
    history,
    scan::truncate,
};

use super::{Action, App, Launch, Resuming, bold, faint};

pub(super) struct Picker {
    folders: Vec<String>,   // the sessions' folders, most recently active first
    accounts: Vec<PathBuf>, // config dirs
    account: usize,
    at: usize,              // the picked one of matches()
    folder: Option<String>, // picked: the input is the first prompt now
    input: LineEdit,        // a filter of the folders, or a path; then the prompt
}

fn home() -> String {
    env::var("HOME").unwrap_or_default()
}

/// A typed path with a leading ~ made the home directory.
fn expand(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", home()),
        _ => path.to_string(),
    }
}

/// Whether the input is a path (completed from the disk), not a filter.
fn is_path(input: &str) -> bool {
    input.starts_with('/') || input.starts_with('~')
}

impl Picker {
    /// A paste into the input; a new filter starts at the first match.
    pub(super) fn paste(&mut self, text: &str) {
        self.input.insert(text);
        if self.folder.is_none() {
            self.at = 0;
        }
    }

    #[cfg(test)]
    pub(super) fn input_text(&self) -> String {
        self.input.text()
    }

    /// Whether the folder is picked: the input is the first prompt.
    pub(super) fn picked(&self) -> bool {
        self.folder.is_some()
    }

    /// The folders the input picks from: for a path, the folders under its
    /// last `/` whose names start with what follows it (dot folders only
    /// when that starts with a dot); else the sessions' folders holding it.
    fn matches(&self) -> Vec<String> {
        let input = self.input.text();
        if !is_path(&input) {
            let needle = input.to_lowercase();
            return self.folders.iter().filter(|f| tilde(f).to_lowercase().contains(&needle)).map(|f| tilde(f)).collect();
        }
        let (dir, prefix) = input.rsplit_once('/').unwrap_or((&input, ""));
        let dir = if dir.is_empty() { "/" } else { dir };
        let mut out: Vec<String> = fs::read_dir(expand(dir))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.starts_with(prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
            .map(|n| format!("{}/{n}", dir.trim_end_matches('/')))
            .collect();
        out.sort();
        out
    }

    /// The folder enter picks: a typed path that is a folder, else the picked match.
    fn chosen(&self) -> Option<String> {
        let input = self.input.text();
        let typed = expand(input.trim_end_matches('/'));
        if is_path(&input) && Path::new(&typed).is_dir() {
            return Some(if typed.is_empty() { "/".into() } else { typed });
        }
        let m = self.matches();
        m.get(self.at.min(m.len().saturating_sub(1))).map(|f| expand(f))
    }
}

impl App {
    /// Opens the picker: the scan's folders, most recently active first, and
    /// every account (the selected session's first).
    pub(super) fn open_picker(&mut self) {
        let mut folders: Vec<String> = Vec::new();
        let mut accounts: Vec<PathBuf> = self.panel_session().map(|a| a.config_dir.clone()).into_iter().collect();
        if let Some(snap) = &self.snap {
            for a in snap.roots.iter().filter_map(|r| snap.by_id.get(r)) {
                if !a.cwd.is_empty() && !folders.contains(&a.cwd) {
                    folders.push(a.cwd.clone());
                }
                if !accounts.contains(&a.config_dir) {
                    accounts.push(a.config_dir.clone());
                }
            }
        }
        for a in crate::limits::accounts(Path::new(&home())) {
            if !accounts.contains(&a.dir) {
                accounts.push(a.dir);
            }
        }
        accounts.retain(|d| !d.as_os_str().is_empty());
        self.picker = Some(Picker { folders, accounts, account: 0, at: 0, folder: None, input: LineEdit::default() });
        self.typing = false;
    }

    /// A key while the picker is open.
    pub(super) fn picker_key(&mut self, k: KeyEvent) -> Action {
        let Some(p) = self.picker.as_mut() else { return Action::None };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match (k.code, &p.folder) {
            (KeyCode::Char('t'), _) if ctrl && !p.accounts.is_empty() => p.account = (p.account + 1) % p.accounts.len(),
            (KeyCode::Esc, None) => self.picker = None,
            (KeyCode::Esc, Some(f)) => {
                let f = tilde(f);
                p.input.set(&f);
                (p.folder, p.at) = (None, 0);
            }
            (KeyCode::Up, None) => p.at = p.at.saturating_sub(1),
            (KeyCode::Down, None) => p.at = (p.at + 1).min(p.matches().len().saturating_sub(1)),
            (KeyCode::Tab, None) => {
                let m = p.matches();
                if let Some(f) = m.get(p.at.min(m.len().saturating_sub(1))) {
                    let f = format!("{f}/");
                    p.input.set(&f);
                    p.at = 0;
                }
            }
            (KeyCode::Enter, None) => match p.chosen() {
                Some(f) => {
                    // `claude --bg` starts only where the account trusts the folder itself.
                    let trusted = |d: &PathBuf| bridge::trusts(d, &f);
                    if !p.accounts.get(p.account).is_some_and(trusted)
                        && let Some(i) = p.accounts.iter().position(trusted)
                    {
                        p.account = i;
                        self.note = format!("account {}: it trusts this folder", tilde(&p.accounts[i].to_string_lossy()));
                    }
                    (p.folder, p.at) = (Some(f), 0);
                    p.input.clear();
                }
                None => self.note = "no such folder: pick one, or type a path (tab completes it)".into(),
            },
            (KeyCode::Enter, Some(_)) if p.input.text().trim().is_empty() => self.note = "type the first prompt: a session shows only once it has one".into(),
            (KeyCode::Enter, Some(_)) => return self.start_picked(),
            _ => {
                if p.input.key(k) && p.folder.is_none() {
                    p.at = 0;
                }
            }
        }
        Action::None
    }

    /// Starts the picked session in the background with its first prompt.
    fn start_picked(&mut self) -> Action {
        if self.resuming.is_some() {
            self.note = "a session is starting or resuming already, wait for it".into();
            return Action::None;
        }
        let Some(p) = self.picker.take() else { return Action::None };
        let (Some(cwd), Some(config_dir)) = (p.folder.clone(), p.accounts.get(p.account).cloned()) else { return Action::None };
        let prompt = p.input.text().trim().to_string();
        let launch = Launch::New(prompt.clone());
        self.note = match bridge::start(&self.claude, &config_dir, &cwd, &launch) {
            Ok(child) => {
                let _ = history::add(&self.bridge, &cwd, &prompt); // recall is a convenience: a failure loses nothing
                self.resuming = Some(Resuming { launch, child, cwd: cwd.clone(), config_dir });
                format!("starting a session in {}…", tilde(&cwd))
            }
            Err(e) => format!("cannot start it: {e}"),
        };
        Action::None
    }

    /// The picker in the tree pane's w columns and h rows.
    pub(super) fn picker_lines(&self, w: usize, h: usize) -> Vec<Line<'static>> {
        let Some(p) = &self.picker else { return Vec::new() };
        let orange = Style::new().fg(Color::Indexed(208));
        let account = p.accounts.get(p.account).map_or("-".into(), |d| tilde(&d.to_string_lossy()));
        let more = if p.accounts.len() > 1 { "  · ctrl+t another" } else { "" };
        let cursor = |text: String, lead: &'static str| Line::from(vec![Span::styled(lead, orange), Span::raw(truncate(&text.replace('\n', "↵"), w.saturating_sub(3))), Span::styled("█", orange)]);
        let mut out = vec![
            Line::styled("new session", orange.add_modifier(Modifier::BOLD)),
            Line::from(vec![Span::styled("account ", faint()), Span::styled(truncate(&account, w.saturating_sub(8)), bold()), Span::styled(more, faint())]),
            Line::raw(""),
        ];
        match &p.folder {
            Some(f) => {
                out.push(Line::from(vec![Span::styled("folder  ", faint()), Span::styled(truncate(&tilde(f), w.saturating_sub(8)), bold())]));
                let trust = match p.accounts.get(p.account) {
                    Some(d) if bridge::trusts(d, f) => Line::styled("        trusted by this account", faint()),
                    _ => Line::styled(truncate("        not trusted by this account: it starts here in the terminal (o), or ctrl+t another", w), orange),
                };
                out.push(trust);
                out.push(Line::raw(""));
                out.push(Line::styled("first prompt:", faint()));
                out.push(cursor(p.input.text(), "> "));
                out.push(Line::raw(""));
                out.push(Line::styled(truncate("enter starts it in the background · esc another folder", w), faint()));
            }
            None => {
                out.push(Line::styled(truncate("folder (type to filter, / or ~ for a path):", w), faint()));
                out.push(cursor(p.input.text(), "› "));
                let m = p.matches();
                let at = p.at.min(m.len().saturating_sub(1));
                let room = h.saturating_sub(out.len()).max(1);
                let from = (at + 1).saturating_sub(room);
                if m.is_empty() {
                    out.push(Line::styled("  no folder matches", faint()));
                }
                for (i, f) in m.iter().enumerate().skip(from).take(room) {
                    let style = if i == at { Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD) } else { Style::new() };
                    out.push(Line::styled(truncate(&format!("  {f}"), w), style));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(folders: &[&str], input: &str) -> Picker {
        let mut p = Picker { folders: folders.iter().map(|f| f.to_string()).collect(), accounts: vec!["/cfg".into()], account: 0, at: 0, folder: None, input: LineEdit::default() };
        p.input.set(input);
        p
    }

    /// Typing filters the sessions' folders; a path lists the folders on disk
    /// under it, and enter takes a typed folder as it is.
    #[test]
    fn the_picker_filters_folders_and_completes_paths() {
        assert_eq!(picker(&["/w/api", "/w/web", "/x/API-old"], "api").matches(), ["/w/api", "/x/API-old"]);
        assert_eq!(picker(&["/w/api"], "").chosen().as_deref(), Some("/w/api"));
        assert_eq!(picker(&["/w/api"], "zzz").chosen(), None);
        let dir = env::temp_dir().join(format!("office-picker-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for d in ["alpha", "beta", ".hidden"] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        fs::write(dir.join("afile"), "").unwrap();
        let root = dir.to_string_lossy().into_owned();
        assert_eq!(picker(&[], &format!("{root}/")).matches(), [format!("{root}/alpha"), format!("{root}/beta")], "folders only, no dot folders");
        assert_eq!(picker(&[], &format!("{root}/a")).matches(), [format!("{root}/alpha")]);
        assert_eq!(picker(&[], &format!("{root}/.")).matches(), [format!("{root}/.hidden")]);
        assert_eq!(picker(&[], &format!("{root}/beta/")).chosen(), Some(format!("{root}/beta")), "a typed folder is taken as it is");
        let _ = fs::remove_dir_all(&dir);
    }
}
