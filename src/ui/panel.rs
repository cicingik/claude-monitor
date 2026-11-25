//! The panel under the tree: the selected session's conversation, read
//! only, and Claude Code's own terminal for it (see embed), opened with p.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    bridge,
    markdown,
    format::{label, short_id},
    scan::{Agent, Status, truncate},
};

use super::{Action, App, Here, Launch, Resuming, View, bold, faint, status_color};

/// A session's conversation, oldest first, as Claude Code shows it: a
/// prompt after `>` as typed, a reply after `●` in markdown, a tool call dim;
/// whole texts wrapped to w. A session with no events yet shows its last reply.
fn log_lines(s: &Agent, w: usize) -> Vec<Line<'static>> {
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
    out
}

/// The most rows the panel gives the note.
const NOTE_LINES: usize = 5;
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
    /// The session the panel shows: the root of the selected row.
    pub(super) fn panel_session(&self) -> Option<&Agent> {
        let mut a = self.agent(&self.selected)?;
        while let Some(p) = self.agent(&a.parent_id) {
            a = p;
        }
        Some(a)
    }

    /// Updates what the panel shows; call it every frame.
    pub fn poll_bridge(&mut self) {
        self.poll_resume();
        self.poll_term();
        self.live = self.panel_session().is_some_and(|a| self.view == View::Tree && a.live);
        let session = self.panel_session().map(|a| a.id.clone()).unwrap_or_default();
        if session != self.log_for {
            (self.log_scroll, self.log_for) = (0, session);
        }
    }

    /// Opens Claude Code's own terminal for the panel's session, in the
    /// panel: `claude attach` of its background session. One no process
    /// holds resumes in the background first and opens once it runs; one
    /// open in a terminal of its own cannot be attached.
    pub(super) fn open_term(&mut self) -> Action {
        // A new session that needs a terminal runs whatever row is selected.
        if let Some(h) = self.here_for.take_if(|h| matches!(h.launch, Launch::New(_))) {
            self.note = "starting it here…".into();
            return Action::RunHere(h);
        }
        let Some(a) = self.panel_session().cloned() else { return Action::None };
        if let Some(h) = self.here_for.take_if(|h| h.launch == Launch::Resume(a.id.clone())) {
            self.note = format!("running {} here…", short_id(&a.id));
            return Action::RunHere(h);
        }
        self.term_for = None;
        match bridge::holder(&a.config_dir, &a.id) {
            bridge::Holder::Background { job } => {
                let [_, pane, _] = self.panes.get();
                let opened = bridge::attach(&self.claude, &a.config_dir, &a.cwd, &job)
                    .and_then(|cmd| crate::embed::Embed::spawn(&a.id, cmd, pane.height.max(10), pane.width.max(40)));
                self.note = match opened {
                    Ok(t) => {
                        self.term = Some(t);
                        "Claude Code's terminal: every key goes to it; esc detaches, ctrl+c interrupts it".into()
                    }
                    Err(e) => format!("cannot open its terminal: {e}"),
                };
            }
            bridge::Holder::Terminal => self.note = "it runs in a terminal of its own: only a background session opens here (exit it there, then p)".into(),
            bridge::Holder::None if self.resuming_for(&a.id) => self.term_for = Some((a.id.clone(), std::time::Instant::now())),
            bridge::Holder::None if self.resuming.is_some() => self.note = "resuming another session, wait for it".into(),
            bridge::Holder::None => {
                self.note = self.resume_in_background(&a);
                if self.resuming.is_some() {
                    self.term_for = Some((a.id.clone(), std::time::Instant::now()));
                }
            }
        }
        Action::None
    }

    /// Closes the terminal once its attach ended, saying what it said last;
    /// opens the one waiting on a resume once the session runs.
    fn poll_term(&mut self) {
        if let Some(t) = self.term.as_mut()
            && !t.running()
        {
            let said = t.said();
            self.term = None;
            self.note = if said.len() < 200 && !said.is_empty() { format!("detached: {said}") } else { "detached: the session runs on in the background".into() };
        }
        let Some((id, since)) = self.term_for.clone() else { return };
        if self.resuming_for(&id) {
            return;
        }
        let held = self.agent(&id).map(|a| bridge::holder(&a.config_dir, &a.id));
        match held {
            // The registry can lag the resume a little.
            Some(bridge::Holder::None) if since.elapsed() < std::time::Duration::from_secs(10) && self.here_for.is_none() => {}
            Some(bridge::Holder::Background { .. }) if self.panel_session().is_some_and(|a| a.id == id) => _ = self.open_term(),
            _ => self.term_for = None,
        }
    }

    /// Starts `claude --resume --bg` for a: what happened, for the note.
    fn resume_in_background(&mut self, a: &Agent) -> String {
        let launch = Launch::Resume(a.id.clone());
        match bridge::start(&self.claude, &a.config_dir, &a.cwd, &launch) {
            Ok(child) => {
                self.resuming = Some(Resuming { launch, child, cwd: a.cwd.clone(), config_dir: a.config_dir.clone() });
                "resuming it in the background…".into()
            }
            Err(e) => format!("cannot resume: {e}"),
        }
    }

    /// Back from claude in office's terminal.
    pub fn ran_here(&mut self, h: &Here, r: std::io::Result<std::process::ExitStatus>) {
        self.note = match (r, &h.launch) {
            (Ok(_), Launch::Resume(s)) => format!("back from {}: its folder is trusted now if you accepted, so p opens it in the background", short_id(s)),
            (Ok(_), Launch::New(_)) => "back from the new session: its folder is trusted now if you accepted, so n starts the next one there in the background".into(),
            (Err(e), _) => format!("cannot run it here: {e}"),
        };
    }

    /// Whether the claude office is starting is for session.
    fn resuming_for(&self, session: &str) -> bool {
        self.resuming.as_ref().is_some_and(|r| matches!(&r.launch, Launch::Resume(s) if s == session))
    }

    /// Takes the end of a resume: what `claude --bg` said, in the note.
    pub(super) fn poll_resume(&mut self) {
        let Some(r) = &mut self.resuming else { return };
        let Ok(Some(status)) = r.child.try_wait() else { return };
        let Resuming { launch, mut child, cwd, config_dir } = self.resuming.take().expect("checked above");
        let mut out = String::new();
        if let Some(mut o) = child.stdout.take() {
            let _ = std::io::Read::read_to_string(&mut o, &mut out);
        }
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut out);
        }
        let said = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · ");
        self.note = match (status.success(), said.is_empty(), &launch) {
            (true, _, Launch::New(_)) => match bridge::started_id(&said) {
                Some(id) => {
                    self.select_started = Some(id.to_string());
                    format!("started {id} in {}: selected once its first turn shows", bridge::tilde(&cwd))
                }
                None => format!("started: {said}"),
            },
            (true, true, _) => "resumed in the background".into(),
            (true, false, _) => format!("resumed: {said}"),
            (false, true, Launch::Resume(_)) => format!("not resumed ({status})"),
            (false, true, Launch::New(_)) => format!("not started ({status})"),
            (false, false, _) => bridge::launch_failed(&said, &config_dir, &cwd, &launch),
        };
        if !status.success() && bridge::needs_terminal(&said) {
            // p runs it here.
            self.here_for = Some(Here { launch, cwd, config_dir });
        }
    }

    /// Scrolls the conversation up (n > 0) or down, within what was last drawn.
    pub(super) fn scroll_log(&mut self, n: isize) {
        self.log_scroll = self.log_scroll.saturating_add_signed(n).min(self.log_max.get());
    }

    /// Half the panel's rows: what page up and down move.
    pub(super) fn page(&self) -> usize {
        usize::from(self.left_heights().1 / 2).max(1)
    }

    /// The panel in h rows: a header (the session and whether a process
    /// holds it), the conversation filling the rows left, newest at the
    /// bottom and scrolled log_scroll lines up, what the session asks, the note.
    pub(super) fn panel_lines(&self, w: usize, h: usize) -> Vec<Line<'static>> {
        let Some(s) = self.panel_session() else { return vec![Line::styled("no session selected", faint())] };
        let (state, style) = match (self.live, self.resuming_for(&s.id)) {
            (_, true) => ("resuming…", faint()),
            (true, _) => ("running · p opens it", Style::new().fg(status_color(Status::Running))),
            (false, _) => ("not running · p resumes and opens it", faint()),
        };
        let name = truncate(&session_name(s), w.saturating_sub(state.chars().count() + 3));
        let head = Line::from(vec![Span::styled(name, bold()), Span::styled(format!(" · {state}"), style)]);
        let waits = asking_lines(s, w);
        let note: Vec<Line<'static>> = wrap(&self.note, w).into_iter().take(NOTE_LINES).map(|l| Line::styled(l, faint())).collect();
        let room = h.saturating_sub(1 + waits.len() + note.len());
        let log = log_lines(s, w);
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
        out.extend(note);
        out
    }
}

/// What the session asks in its transcript (a question, a plan): answered in its terminal.
fn asking_lines(s: &Agent, w: usize) -> Vec<Line<'static>> {
    if s.asking.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Line::styled("asks you · p to answer:", Style::new().fg(Color::Indexed(208)).add_modifier(Modifier::BOLD))];
    let lines = markdown::render(&s.asking, w);
    let more = lines.len() > ASKING_LINES;
    out.extend(lines.into_iter().take(if more { ASKING_LINES - 1 } else { ASKING_LINES }));
    if more {
        out.push(Line::styled("…", faint()));
    }
    out
}
