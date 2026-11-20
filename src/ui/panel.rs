//! The prompt panel under the tree: the bridge to its session, its keys and its lines.

use std::time::SystemTime;

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    bridge::{self, Answer},
    history,
    commands,
    markdown,
    format::{label, short_id},
    scan::{Agent, Status, truncate},
};

use super::{Action, App, Here, Recall, Resuming, View, bold, faint, status_color};

/// A session's conversation, oldest first, as Claude Code shows it: a
/// prompt after `>` as typed, a reply after `●` in markdown, a tool call dim;
/// whole texts wrapped to w. A session with no events yet shows its last reply.
/// A prompt left for it and not taken yet comes last, dim, marked so.
fn log_lines(s: &Agent, queued: Option<&str>, w: usize) -> Vec<Line<'static>> {
    let text_w = w.saturating_sub(2).max(1);
    let block = |out: &mut Vec<Line<'static>>, mark: &'static str, mark_style: Style, body: Vec<Line<'static>>| {
        out.push(Line::raw(""));
        for (i, l) in body.into_iter().enumerate() {
            let mut spans = vec![Span::styled(if i == 0 { mark } else { "  " }, mark_style)];
            spans.extend(l.spans);
            out.push(Line::from(spans));
        }
    };
    let prompt = |text: &str| wrap(text, text_w).into_iter().map(|l| Line::styled(l, Style::new().fg(Color::Indexed(250)))).collect();
    let mut out = Vec::new();
    for e in &s.events {
        let text = if e.text.is_empty() { e.summary.as_str() } else { e.text.as_str() };
        match e.kind {
            "prompt" => block(&mut out, "> ", faint(), prompt(text)),
            "text" => block(&mut out, "● ", Style::new(), markdown::render(text, text_w)),
            "tool_use" => out.push(Line::from(vec![Span::styled("● ", Style::new().fg(Color::Indexed(42))), Span::styled(truncate(text, text_w), faint())])),
            _ => out.push(Line::styled(truncate(&format!("  └ {text}"), w), faint())),
        }
    }
    if out.is_empty() && !s.last_text.is_empty() {
        block(&mut out, "● ", Style::new(), markdown::render(&s.last_text, text_w));
    }
    if let Some(q) = queued {
        block(&mut out, "> ", faint(), wrap(q, text_w).into_iter().map(|l| Line::styled(l, faint())).collect());
        out.push(Line::styled(truncate("  not taken yet · ctrl+x takes it back", w), faint()));
    }
    out
}

/// The most rows the panel gives the note, the command menu and the prompt.
const NOTE_LINES: usize = 5;
const MENU_LINES: usize = 6;
const PROMPT_LINES: usize = 4;
/// The most rows what the session asks takes, so the conversation keeps some.
const ASKING_LINES: usize = 12;

/// A session as the panel names it: its label, with its project unless the
/// label (an untitled session's) has it already.
fn session_name(s: &Agent) -> String {
    let (l, p) = (label(s), format!("({})", s.project));
    if l.contains(&p) { l } else { format!("{l} {p}") }
}

/// text in lines of at most w characters, broken at spaces where it can.
fn wrap(text: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut out = Vec::new();
    for para in text.lines() {
        let indent: String = para.chars().take_while(|c| *c == ' ').collect();
        let mut line = String::new();
        for word in para.split_whitespace() {
            let mut word: Vec<char> = word.chars().collect();
            loop {
                let len = line.chars().count();
                let sep = usize::from(len > indent.len());
                if len + sep + word.len() <= w {
                    if sep == 1 {
                        line.push(' ');
                    } else if line.is_empty() {
                        line.push_str(&indent);
                    }
                    line.extend(word.iter());
                    break;
                }
                if len > indent.len() {
                    out.push(std::mem::take(&mut line));
                    continue;
                }
                let fit = w.saturating_sub(len).max(1).min(word.len());
                line.extend(word.drain(..fit));
                out.push(std::mem::take(&mut line));
                if word.is_empty() {
                    break;
                }
            }
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

impl App {
    /// The session the prompt panel talks to: the root of the selected row.
    pub(super) fn panel_session(&self) -> Option<&Agent> {
        let mut a = self.agent(&self.selected)?;
        while let Some(p) = self.agent(&a.parent_id) {
            a = p;
        }
        Some(a)
    }

    /// Reads what the panel shows from the bridge; call it every frame.
    pub fn poll_bridge(&mut self) {
        self.poll_resume();
        self.live = self.panel_session().is_some_and(|a| self.view == View::Tree && bridge::live(&a.config_dir, &a.id));
        let id = self.panel_session().map(|a| a.id.clone()).filter(|_| self.view == View::Tree);
        (self.ask, self.queued) = match id {
            Some(id) => (bridge::ask(&self.bridge, &id), bridge::queued(&self.bridge, &id).map(|m| commands::typed(&m).unwrap_or(m))),
            None => (None, None),
        };
        let key = self.panel_session().map(|a| (a.config_dir.clone(), a.cwd.clone()));
        if key.is_some() && key != self.commands_for {
            let (dir, cwd) = key.clone().expect("checked above");
            (self.commands, self.commands_for) = (commands::list(&dir, std::path::Path::new(&cwd)), key);
        }
        let session = self.panel_session().map(|a| a.id.clone()).unwrap_or_default();
        if session != self.log_for {
            (self.log_scroll, self.log_for) = (0, session);
        }
        let id = self.ask.as_ref().map(|a| a.id.clone()).unwrap_or_default();
        if id != self.picks_for {
            (self.picks, self.picks_for) = (Vec::new(), id);
        }
    }

    /// Resumes the panel's session in the background, unless a process holds
    /// it already, and gives the keys to its panel.
    pub(super) fn resume(&mut self) -> Action {
        let Some(a) = self.panel_session().cloned() else { return Action::None };
        if let Some(h) = self.here_for.take().filter(|h| h.session == a.id) {
            self.note = format!("running {} here…", short_id(&a.id));
            return Action::RunHere(h);
        }
        self.typing = true;
        if self.resuming.is_some() {
            self.note = "resuming a session already, wait for it".into();
        } else if bridge::live(&a.config_dir, &a.id) {
            self.note = "running already: type a prompt".into();
        } else {
            self.note = match bridge::resume(&a.config_dir, &a.cwd, &a.id) {
                Ok(child) => {
                    self.resuming = Some(Resuming { session: a.id.clone(), child, cwd: a.cwd.clone(), config_dir: a.config_dir.clone() });
                    "resuming in the background…".into()
                }
                Err(e) => format!("cannot resume: {e}"),
            };
        }
        Action::None
    }

    /// Back from `claude --resume` in office's terminal.
    pub fn ran_here(&mut self, h: &Here, r: std::io::Result<std::process::ExitStatus>) {
        self.note = match r {
            Ok(_) => format!("back from {}: its folder is trusted now if you accepted, so o resumes it in the background", short_id(&h.session)),
            Err(e) => format!("cannot run it here: {e}"),
        };
    }

    /// Takes the end of a resume: what `claude --bg` said, in the note.
    pub(super) fn poll_resume(&mut self) {
        let Some(r) = &mut self.resuming else { return };
        let Ok(Some(status)) = r.child.try_wait() else { return };
        let Resuming { session, mut child, cwd, config_dir } = self.resuming.take().expect("checked above");
        let mut out = String::new();
        if let Some(mut o) = child.stdout.take() {
            let _ = std::io::Read::read_to_string(&mut o, &mut out);
        }
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut out);
        }
        let said = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · ");
        self.note = match (status.success(), said.is_empty()) {
            (true, true) => "resumed in the background: type a prompt".into(),
            (true, false) => format!("resumed: {said}"),
            (false, true) => format!("not resumed ({status})"),
            (false, false) => bridge::resume_failed(&said, &config_dir, &cwd, &session),
        };
        if !status.success() && bridge::needs_terminal(&said) {
            // The keys go back to the tree, where o runs it here.
            (self.here_for, self.typing) = (Some(Here { session, cwd, config_dir }), false);
        }
    }

    /// The question of the waiting AskUserQuestion call answered next, if any.
    fn question(&self) -> Option<&bridge::Question> {
        self.ask.as_ref()?.questions.get(self.picks.len())
    }

    /// Answers the current question; the last answer sends them all.
    fn pick(&mut self, session: &str, answer: String) {
        self.picks.push(answer);
        let Some(ask) = self.ask.clone() else { return };
        if self.picks.len() < ask.questions.len() {
            return;
        }
        let answers: Vec<(String, String)> = ask.questions.iter().map(|q| q.text.clone()).zip(self.picks.drain(..)).collect();
        let said = answers.iter().map(|(_, a)| a.as_str()).collect::<Vec<_>>().join(" · ");
        self.note = match bridge::answer(&self.bridge, session, &ask, Answer::Answers(answers)) {
            Ok(()) => format!("answered: {said}"),
            Err(e) => format!("not answered: {e}"),
        };
        self.ask = None;
    }

    /// A key while typing in the panel; q, r and the tree's keys are text here.
    pub(super) fn panel_key(&mut self, k: KeyEvent) -> Action {
        let Some(id) = self.panel_session().map(|a| a.id.clone()) else {
            self.typing = false;
            return Action::None;
        };
        let typed = self.input.text();
        let menu: Vec<String> = commands::matching(&self.commands, &typed).iter().map(|c| c.name.clone()).collect();
        let picked = menu.get(self.menu_at.min(menu.len().saturating_sub(1))).cloned();
        match k.code {
            KeyCode::Esc => (self.typing, self.recall) = (false, None),
            KeyCode::Up if self.recall.is_none() && !menu.is_empty() => self.menu_at = (self.menu_at.min(menu.len() - 1) + menu.len() - 1) % menu.len(),
            KeyCode::Down if self.recall.is_none() && !menu.is_empty() => self.menu_at = (self.menu_at.min(menu.len() - 1) + 1) % menu.len(),
            KeyCode::Char('x') if k.modifiers.contains(KeyModifiers::CONTROL) => self.take_back(&id),
            KeyCode::Up => self.recall_step(true),
            KeyCode::Down => self.recall_step(false),
            KeyCode::Tab if picked.is_some() => self.input.set(&format!("/{} ", picked.expect("checked above"))),
            KeyCode::PageUp => self.scroll_log(self.page() as isize),
            KeyCode::PageDown => self.scroll_log(-(self.page() as isize)),
            // A question: a digit picks its option, typed text and enter is an answer of one's own.
            KeyCode::Char(c @ '1'..='9') if self.input.is_empty() && self.question().is_some_and(|q| (c as usize - '0' as usize) <= q.options.len()) => {
                let label = self.question().expect("checked above").options[c as usize - '1' as usize].0.clone();
                self.pick(&id, label);
            }
            KeyCode::Enter if self.question().is_some() && !typed.trim().is_empty() => {
                self.input.clear();
                self.pick(&id, typed.trim().to_string());
            }
            KeyCode::Char(c @ '1'..='3') if self.input.is_empty() && self.ask.as_ref().is_some_and(|a| a.questions.is_empty()) => {
                let a = [Answer::Allow, Answer::AllowAlways, Answer::Deny][c as usize - '1' as usize].clone();
                let ask = self.ask.take().expect("checked above");
                self.note = match bridge::answer(&self.bridge, &id, &ask, a) {
                    Ok(()) => format!("answered: {}", ["yes", "yes, don't ask again", "no"][c as usize - '1' as usize]),
                    Err(e) => format!("not answered: {e}"),
                };
            }
            // Enter on the menu runs the picked command, as Claude Code's own does.
            KeyCode::Enter if !typed.trim().is_empty() => self.send(&id, &picked.map_or(typed, |n| format!("/{n}"))),
            _ => {
                if self.input.key(k) {
                    (self.menu_at, self.recall) = (0, None);
                }
            }
        }
        Action::None
    }

    /// Takes back the prompt the session has not taken yet, into the box
    /// when nothing is typed there.
    fn take_back(&mut self, session: &str) {
        self.note = match bridge::cancel(&self.bridge, session) {
            Ok(Some(m)) => {
                self.queued = None;
                if self.input.is_empty() {
                    self.input.set(&commands::typed(&m).unwrap_or(m));
                }
                "taken back: the session will not see it".into()
            }
            Ok(None) => "nothing to take back: no prompt is waiting".into(),
            Err(e) => format!("cannot take it back: {e}"),
        };
    }

    /// One step of the walk through the project's prompts: older begins it
    /// at the newest; newer past the newest ends it, the draft back.
    fn recall_step(&mut self, older: bool) {
        let r = match self.recall.take() {
            Some(r) => r,
            None if !older => return,
            None => {
                let Some(s) = self.panel_session() else { return };
                let list = history::load(&s.config_dir, &self.bridge, &s.cwd);
                Recall { at: list.len(), list, draft: self.input.text() }
            }
        };
        let at = if older { r.at.saturating_sub(1) } else { r.at + 1 };
        match r.list.get(at) {
            Some(p) => {
                self.input.set(p);
                self.recall = Some(Recall { at, ..r });
            }
            None if older => {} // no prompts
            None => self.input.set(&r.draft),
        }
    }

    /// Leaves the prompt for the session, framed by commands::message; a
    /// built-in stays typed, with the note saying why.
    fn send(&mut self, session: &str, prompt: &str) {
        let message = match commands::message(&self.commands, prompt) {
            Ok(m) => m,
            Err(why) => {
                self.note = why;
                return;
            }
        };
        self.note = match bridge::send(&self.bridge, session, &message) {
            Ok(true) => "sent".into(),
            Ok(false) => "queued: the session takes it once it ends its turn".into(),
            Err(e) => format!("not sent: {e}"),
        };
        if let (Some(cwd), Some(typed)) = (self.agent(session).map(|a| a.cwd.clone()), commands::typed(&message)) {
            let _ = history::add(&self.bridge, &cwd, &typed); // recall is a convenience: a failure loses no prompt
        }
        self.input.clear();
        self.recall = None;
        self.queued = commands::typed(&message);
        self.log_scroll = 0;
    }

    /// Scrolls the conversation up (n > 0) or down, within what was last drawn.
    pub(super) fn scroll_log(&mut self, n: isize) {
        self.log_scroll = self.log_scroll.saturating_add_signed(n).min(self.log_max.get());
    }

    /// Half the panel's rows: what page up and down move.
    fn page(&self) -> usize {
        usize::from(self.left_heights().1 / 2).max(1)
    }

    /// The prompt panel in h rows, laid out as Claude Code's own: a header
    /// (the session and whether a process holds it), the conversation filling
    /// the rows left, newest at the bottom and scrolled log_scroll lines up,
    /// what the session waits on, the command menu, the input box, the note.
    pub(super) fn panel_lines(&self, w: usize, h: usize) -> Vec<Line<'static>> {
        let orange = Style::new().fg(Color::Indexed(208));
        let Some(s) = self.panel_session() else { return vec![Line::styled("no session selected", faint())] };
        let (state, style) = match (self.live, self.resuming.as_ref().is_some_and(|r| r.session == s.id)) {
            (_, true) => ("resuming…", faint()),
            (true, _) => ("running", Style::new().fg(status_color(Status::Running))),
            (false, _) => ("not running · o resumes it", faint()),
        };
        let name = truncate(&session_name(s), w.saturating_sub(state.chars().count() + 3));
        let head = Line::from(vec![Span::styled(name, bold()), Span::styled(format!(" · {state}"), style)]);
        if !self.hooks {
            return vec![
                head,
                Line::styled("hooks not installed: quit, run", orange),
                Line::styled("  office hook install", orange),
                Line::styled("then restart the Claude Code sessions", faint()),
            ];
        }
        let waits = self.waiting_lines(s, w);
        let mut bottom = if self.typing { self.menu_lines(w) } else { Vec::new() };
        bottom.extend(self.input_box(w));
        bottom.extend(wrap(&self.note, w).into_iter().take(NOTE_LINES).map(|l| Line::styled(l, faint())));
        let room = h.saturating_sub(1 + waits.len() + bottom.len());
        let log = log_lines(s, self.queued.as_deref(), w);
        let max = log.len().saturating_sub(room);
        self.log_max.set(max);
        let up = self.log_scroll.min(max);
        let (start, end) = ((log.len() - up).saturating_sub(room), log.len() - up);
        let mut shown = log[start..end].to_vec();
        if start > 0 && room > 1 {
            shown[0] = Line::styled(format!("↑ {start} more · pgup"), faint());
        }
        if up > 0 && room > 1 {
            *shown.last_mut().expect("room > 1") = Line::styled(format!("↓ {up} newer · pgdn"), faint());
        }
        let mut out = vec![head];
        out.extend(std::iter::repeat_n(Line::raw(""), room - shown.len())); // the newest stays at the bottom
        out.extend(shown);
        out.extend(waits);
        out.extend(bottom);
        out
    }

    /// What the session waits on, over the input box: its permission
    /// request, its question (one at a time) with the options to pick, else
    /// what it asks in its transcript (a question, a plan), or a prompt not yet taken.
    fn waiting_lines(&self, s: &Agent, w: usize) -> Vec<Line<'static>> {
        let orange = Style::new().fg(Color::Indexed(208));
        let mut out = Vec::new();
        let asked = self.question().map(|q| (q, self.picks.len() + 1, self.ask.as_ref().map_or(0, |a| a.questions.len())));
        match &self.ask {
            Some(_) if asked.is_some() => {
                let (q, n, of) = asked.expect("checked above");
                let head = if of > 1 { format!("asks you ({n} of {of}):") } else { "asks you:".to_string() };
                out.push(Line::styled(head, orange.add_modifier(Modifier::BOLD)));
                out.extend(wrap(&q.text, w).into_iter().map(|l| Line::styled(l, bold())));
                for (i, (label, desc)) in q.options.iter().enumerate() {
                    let o = if desc.is_empty() { format!("  {}. {label}", i + 1) } else { format!("  {}. {label} — {desc}", i + 1) };
                    out.extend(wrap(&o, w).into_iter().map(Line::raw));
                }
                let many = if q.multi { " (one; or type them, comma-separated)" } else { "" };
                out.push(Line::styled(truncate(&format!("1-{} pick{many} · or type your own + enter", q.options.len()), w), faint()));
            }
            Some(a) => {
                let text: Vec<char> = format!("asks: {}", a.summary).chars().collect();
                for c in text.chunks(w.max(1)).take(2) {
                    out.push(Line::styled(c.iter().collect::<String>(), orange.add_modifier(Modifier::BOLD)));
                }
                let left = bridge::HOLD.saturating_sub(SystemTime::now().duration_since(a.at).unwrap_or_default()).as_secs();
                out.push(Line::raw(truncate("1 yes · 2 yes, don't ask again · 3 no", w)));
                out.push(Line::styled(truncate(&format!("or in its own dialog · answerable here {}m more", left.div_ceil(60)), w), faint()));
            }
            None if !s.asking.is_empty() => {
                out.push(Line::styled("asks you:", orange.add_modifier(Modifier::BOLD)));
                let lines = markdown::render(&s.asking, w);
                let more = lines.len() > ASKING_LINES;
                out.extend(lines.into_iter().take(if more { ASKING_LINES - 1 } else { ASKING_LINES }));
                if more {
                    out.push(Line::styled("…", faint()));
                }
            }
            None => {}
        }
        out
    }

    /// The prompt in a rounded box, as Claude Code draws its own; while the
    /// tree has the keys, the box says how to give them to it.
    fn input_box(&self, w: usize) -> Vec<Line<'static>> {
        let edge = Style::new().fg(if self.typing { Color::Indexed(208) } else { Color::Indexed(240) });
        let inner = w.saturating_sub(4);
        let rows = if self.typing { self.prompt_lines(inner) } else { vec![Line::styled(truncate("p to type a prompt", inner), faint())] };
        let mut out = vec![Line::styled(format!("╭{}╮", "─".repeat(w.saturating_sub(2))), edge)];
        for row in rows {
            let pad = inner.saturating_sub(row.width());
            let mut spans = vec![Span::styled("│ ", edge)];
            spans.extend(row.spans);
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(" │", edge));
            out.push(Line::from(spans));
        }
        out.push(Line::styled(format!("╰{}╯", "─".repeat(w.saturating_sub(2))), edge));
        out
    }

    /// The commands matching the prompt, the picked one marked, in a window
    /// of MENU_LINES that keeps it in sight.
    fn menu_lines(&self, w: usize) -> Vec<Line<'static>> {
        let menu = commands::matching(&self.commands, &self.input.text());
        let at = self.menu_at.min(menu.len().saturating_sub(1));
        let from = (at + 1).saturating_sub(MENU_LINES);
        let picked = Style::new().fg(Color::Indexed(208)).add_modifier(Modifier::BOLD);
        menu.iter().enumerate().skip(from).take(MENU_LINES).map(|(i, c)| {
            let (mark, style) = if i == at { ("› ", picked) } else { ("  ", Style::new()) };
            let name = format!("{mark}/{}", c.name);
            let about = if c.file.is_none() { "terminal only".to_string() } else { c.about.clone() };
            let gap = w.saturating_sub(name.chars().count() + 2);
            Line::from(vec![Span::styled(truncate(&name, w), style), Span::styled(if gap > 3 { format!("  {}", truncate(&about, gap)) } else { String::new() }, faint())])
        }).collect()
    }

    /// The prompt wrapped to w, the cursor on the char it is before, in up
    /// to PROMPT_LINES rows ending at the cursor's.
    fn prompt_lines(&self, w: usize) -> Vec<Line<'static>> {
        let orange = Style::new().fg(Color::Indexed(208));
        let (rows, row, col) = self.input.rows(w.saturating_sub(2));
        let from = (row + 1).saturating_sub(PROMPT_LINES);
        rows.iter().enumerate().skip(from).take(PROMPT_LINES).map(|(i, r)| {
            let lead = Span::styled(if i == 0 { "> " } else { "  " }, orange);
            if i != row {
                return Line::from(vec![lead, Span::raw(r.clone())]);
            }
            let chars: Vec<char> = r.chars().collect();
            let (before, rest) = chars.split_at(col.min(chars.len()));
            let cursor = match rest.first() {
                Some(c) => Span::styled(c.to_string(), Style::new().add_modifier(Modifier::REVERSED)),
                None => Span::styled("█", orange),
            };
            Line::from(vec![lead, Span::raw(before.iter().collect::<String>()), cursor, Span::raw(rest.iter().skip(1).collect::<String>())])
        }).collect()
    }
}
