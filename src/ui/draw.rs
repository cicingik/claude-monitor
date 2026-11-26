//! Draws the tree view: the header, the tree, the session panel and the detail tabs.

use std::collections::{BTreeMap, HashMap};

use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::{
    bridge::Holder,
    format::{age, agent_type, clock, fit, hour_minute, human_tokens, label, local, short_duration, short_id, short_model, usage_detail},
    limits::{AccountUsage, Limit},
    scan::{Agent, EdgeKind, Kind, Snapshot, Status, Usage, days_from_civil, truncate},
};

use super::{App, TABS, bold, doing, faint, glyph, status_color};

fn header_style() -> Style {
    Style::new().fg(Color::Indexed(212)).add_modifier(Modifier::BOLD)
}

/// A session's row in the tree: `[account](directory) title`, its short id
/// when it has no title yet.
fn session_row(a: &Agent) -> String {
    let title = if a.title.is_empty() { short_id(&a.id).to_string() } else { a.title.clone() };
    format!("[{}]({}) {title}", a.config, a.project)
}

/// The bubbles help line: "key desc • key desc".
fn help(keys: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, (k, d)) in keys.iter().enumerate() {
        if i > 0 {
            out.push(Span::styled(" • ", faint()));
        }
        out.push(Span::styled(k.to_string(), Style::new().fg(Color::Indexed(250))));
        out.push(Span::styled(format!(" {d}"), faint()));
    }
    out
}

impl App {
    /// Draws the tree view.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        self.height = area.height;
        // The pane may have changed since the cursor last moved, while the office was shown.
        self.scroll_to_cursor();
        self.draw_tree(f, area);
        self.draw_selection(f);
    }

    fn line(f: &mut Frame, area: Rect, y: u16, spans: Vec<Span<'static>>) {
        if y < area.bottom() {
            f.render_widget(Paragraph::new(Line::from(spans)), Rect::new(area.x, y, area.width, 1));
        }
    }

    fn draw_tree(&self, f: &mut Frame, area: Rect) {
        let header = match &self.snap {
            None => vec![Span::styled("claude-monitor", header_style()), Span::raw("  scanning…")],
            Some(snap) => {
                let running = snap.by_id.values().filter(|a| a.status == Status::Running).count();
                let waiting = snap.by_id.values().filter(|a| a.status == Status::Waiting).count();
                let since = if self.since > 0 { short_duration(self.since) } else { "all".into() };
                vec![
                    Span::styled("claude-monitor", header_style()),
                    Span::raw(format!(
                        "  sessions {} · agents {} · running {running} · waiting {waiting}  │  scanned {}  │  since {since}",
                        snap.roots.len(),
                        snap.by_id.len(),
                        clock(snap.scanned_at)
                    )),
                ]
            }
        };
        Self::line(f, area, area.y, header);
        let body_h = area.height.saturating_sub(2).max(4).min(area.height.saturating_sub(1));
        let lw = (area.width / 2).max(30).min(area.width.saturating_sub(24).max(30)).min(area.width);
        let rw = area.width.saturating_sub(lw);
        let block = || Block::new().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::new().fg(Color::Indexed(240)));
        let (tree_h, panel_h) = self.left_heights();
        let tree_h = tree_h.min(body_h);
        let left = Rect::new(area.x, area.y + 1, lw, tree_h);
        // Claude Code's terminal takes the panel's rows across the whole width.
        let (panel_w, right_h) = if self.term.is_some() { (area.width, tree_h) } else { (lw, body_h) };
        let panel = Rect::new(area.x, area.y + 1 + tree_h, panel_w, panel_h.min(body_h - tree_h));
        let right = Rect::new(area.x + lw, area.y + 1, rw, right_h);
        self.panes.set([left, panel, right].map(|r| r.inner(Margin::new(1, 1))));
        let (tw, th) = (usize::from(lw.saturating_sub(2)), usize::from(tree_h.saturating_sub(2)));
        match &self.picker {
            Some(_) => f.render_widget(Paragraph::new(self.picker_lines(tw, th)).block(block().border_style(Style::new().fg(Color::Indexed(208)))), left),
            None => f.render_widget(Paragraph::new(self.tree_lines(tw)).block(block()), left),
        }
        if let Some(t) = &self.term {
            let orange = Style::new().fg(Color::Indexed(208));
            let name = self.agent(&t.session).map(session_row).unwrap_or_default();
            f.render_widget(block().border_style(orange).title(Span::styled(format!(" claude · {} ", truncate(&name, usize::from(panel_w).saturating_sub(16))), orange)), panel);
            t.draw(f, panel.inner(Margin::new(1, 1)));
        } else {
            let panel_block = block().title(Span::styled(" session ", Style::new().fg(Color::Indexed(240))));
            let lines = self.panel_lines(usize::from(lw.saturating_sub(2)), usize::from(panel.height.saturating_sub(2)));
            f.render_widget(Paragraph::new(lines).block(panel_block), panel);
        }
        if rw > 2 {
            f.render_widget(Paragraph::new(self.detail_lines(usize::from(rw - 2))).block(block()), right);
        }
        let keys: &[(&str, &str)] = if self.term.is_some() {
            &[("keys", "go to Claude Code"), ("ctrl+c", "interrupt it"), ("esc", "detach, back to the tree (the session runs on)")]
        } else if self.picker.as_ref().is_some_and(|p| p.picked()) {
            &[("enter", "start it"), ("ctrl+t", "account"), ("esc", "another folder"), ("ctrl+c", "quit")]
        } else if self.picker.is_some() {
            &[("↑/↓", "pick"), ("tab", "complete"), ("enter", "this folder"), ("ctrl+t", "account"), ("esc", "close"), ("ctrl+c", "quit")]
        } else {
            &[("↑/k", "up"), ("↓/j", "down"), ("enter", "collapse/expand"), ("p", "open in Claude Code"), ("pgup/pgdn", "scroll the session"), ("n", "new session"), ("tab", "next tab"), ("1-3", "tab"), ("v", "office"), ("r", "rescan"), ("q", "quit")]
        };
        Self::line(f, area, area.y + 1 + body_h, help(keys));
    }

    pub(super) fn tree_lines(&self, w: usize) -> Vec<Line<'static>> {
        let rows = self.rows();
        if rows.is_empty() {
            return vec![Line::styled("no sessions in the time window", faint())];
        }
        let now = self.snap.as_ref().map_or(0, |s| s.scanned_at);
        let end = (self.offset + self.tree_height()).min(rows.len());
        (self.offset..end)
            .map(|i| {
                let (a, depth) = rows[i];
                let caret = match (a.children.is_empty(), self.collapsed.contains(&a.id)) {
                    (true, _) => " ",
                    (false, true) => "▸",
                    (false, false) => "▾",
                };
                let prefix = format!("{}{caret} ", "  ".repeat(depth));
                // Model and tokens are in the detail tabs; a row names the agent and its age.
                // How a running session is held: bg opens in the panel with p, tty runs in a terminal of its own.
                let held = match a.held {
                    Holder::Background { .. } => " bg ",
                    Holder::Terminal => " tty",
                    Holder::None => "",
                };
                let mut cols = format!("{held} {:>4}", age(now, a.last_activity));
                if w >= 60 && a.kind == Kind::Subagent {
                    cols = format!(" {}{cols}", fit(agent_type(a), 15));
                }
                let name = if a.kind == Kind::Session { session_row(a) } else { label(a) };
                let title = fit(&name, w.saturating_sub(prefix.chars().count() + 2 + cols.chars().count()));
                if i == self.cursor {
                    let sel = Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD);
                    return Line::styled(format!("{prefix}{} {title}{cols}", a.status.glyph()), sel);
                }
                Line::from(vec![Span::styled(prefix, faint()), glyph(a.status), Span::raw(format!(" {title}")), Span::styled(cols, faint())])
            })
            .collect()
    }

    fn detail_lines(&self, w: usize) -> Vec<Line<'static>> {
        let mut tabs = Vec::new();
        for (i, n) in TABS.iter().enumerate() {
            if i > 0 {
                tabs.push(Span::raw(" "));
            }
            let style = if i == self.tab { Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD) } else { faint() };
            tabs.push(Span::styled(format!(" {} {n} ", i + 1), style));
        }
        let mut out = vec![Line::from(tabs), Line::styled("─".repeat(w), faint())];
        if self.tab == 2 {
            out.extend(self.accounts(w));
        }
        let (Some(snap), Some(a)) = (&self.snap, self.agent(&self.selected)) else {
            out.push(Line::styled("nothing selected", faint()));
            return out;
        };
        out.extend(match self.tab {
            1 => self.interactions(snap, a),
            2 => self.usage(snap, a, w),
            _ => self.activity(snap, a),
        });
        out
    }

    fn activity(&self, snap: &Snapshot, a: &Agent) -> Vec<Line<'static>> {
        let mut out = vec![
            Line::from(vec![glyph(a.status), Span::raw(" "), Span::styled(label(a), bold())]),
            Line::raw(format!("{} · {} · {} · {} · {}", agent_type(a), short_model(&a.model), a.status.name(), a.project, a.config)),
            Line::styled(format!("id {}", a.id), faint()),
            Line::raw(format!("turns {} · tokens {} ({})", a.turns, human_tokens(a.usage.total()), usage_detail(a.usage))),
            Line::raw(format!("last activity {} ago", age(snap.scanned_at, a.last_activity))),
        ];
        if let Some(now) = doing(a.status) {
            out.push(Line::styled(format!("{now} {}", a.last_tool), Style::new().fg(status_color(a.status)).add_modifier(Modifier::BOLD)));
        }
        out.push(Line::raw(""));
        out.push(Line::styled("Recent events", bold()));
        if a.events.is_empty() {
            out.push(Line::styled("  (none)", faint()));
        }
        for e in a.events.iter().rev() {
            let kind = match e.kind {
                "tool_use" => "tool",
                "text" => "reply",
                k => k,
            };
            out.push(Line::from(vec![Span::styled(clock(e.at), faint()), Span::raw(format!(" {} {}", fit(kind, 8), e.summary))]));
        }
        out
    }

    /// The short display form of an agent id in edges and the graph.
    fn name(&self, id: &str) -> String {
        match self.agent(id) {
            Some(a) => format!("{} {}", short_id(id), agent_type(a)),
            None => id.into(),
        }
    }

    fn interactions(&self, snap: &Snapshot, a: &Agent) -> Vec<Line<'static>> {
        let mut out = vec![Line::styled(format!("Edges of {}", short_id(&a.id)), bold())];
        let before = out.len();
        for e in &snap.edges {
            let s = if e.from == a.id {
                format!("→ {:<8} to   {}", e.kind.name(), self.name(&e.to))
            } else if e.to == a.id {
                format!("← {:<8} from {}", e.kind.name(), self.name(&e.from))
            } else {
                continue;
            };
            let mut l = vec![Span::raw(format!("  {s}"))];
            if !e.label.is_empty() && e.kind != EdgeKind::Handback {
                l.push(Span::styled(format!(" \"{}\"", e.label), faint()));
            }
            l.push(Span::styled(format!(" {}", hour_minute(e.at)), faint()));
            out.push(Line::from(l));
        }
        if out.len() == before {
            out.push(Line::styled("  (none)", faint()));
        }
        let Some(root) = snap.by_id.get(&a.session_id) else { return out };
        out.push(Line::raw(""));
        out.push(Line::styled("Session graph", bold()));
        out.extend(self.graph(snap, root));
        out
    }

    /// The session tree with spawn labels on the branches and each agent's
    /// outgoing messages and handbacks as annotations.
    fn graph(&self, snap: &Snapshot, root: &Agent) -> Vec<Line<'static>> {
        let mut spawn_label: HashMap<&str, &str> = HashMap::new();
        let mut notes: HashMap<&str, BTreeMap<String, usize>> = HashMap::new();
        for e in &snap.edges {
            match e.kind {
                EdgeKind::Spawn => _ = spawn_label.insert(&e.to, &e.label),
                EdgeKind::Message => *notes.entry(&e.from).or_default().entry(format!("⇢msg {}", short_id(&e.to))).or_default() += 1,
                EdgeKind::Handback => *notes.entry(&e.from).or_default().entry(format!("⇠handback {}", short_id(&e.to))).or_default() += 1,
            }
        }
        let annotate = |id: &str| -> Option<Span<'static>> {
            let n = notes.get(id)?;
            let parts: Vec<String> = n.iter().map(|(k, c)| format!("[{k} ×{c}]")).collect();
            Some(Span::styled(format!(" {}", parts.join(" ")), faint()))
        };
        let mut first = vec![glyph(root.status), Span::raw(format!(" {} session", short_id(&root.id)))];
        first.extend(annotate(&root.id));
        let mut out = vec![Line::from(first)];
        fn rec(app: &App, snap: &Snapshot, a: &Agent, prefix: &str, labels: &HashMap<&str, &str>, annotate: &dyn Fn(&str) -> Option<Span<'static>>, out: &mut Vec<Line<'static>>) {
            for (i, c) in a.children.iter().enumerate() {
                let Some(c) = snap.by_id.get(c) else { continue };
                let last = i == a.children.len() - 1;
                let (branch, next) = if last { ("└─", format!("{prefix}   ")) } else { ("├─", format!("{prefix}│  ")) };
                let mut l = vec![
                    Span::styled(format!("{prefix}{branch}spawn \"{}\"─▶ ", labels.get(c.id.as_str()).copied().unwrap_or("")), faint()),
                    glyph(c.status),
                    Span::raw(format!(" {}", app.name(&c.id))),
                ];
                l.extend(annotate(&c.id));
                out.push(Line::from(l));
                rec(app, snap, c, &next, labels, annotate, out);
            }
        }
        rec(self, snap, root, "", &spawn_label, &annotate, &mut out);
        out
    }

    /// The Accounts section: per account, a bar for its 5-hour and one for
    /// its 7-day limit, in the session bars' style, or one line saying why
    /// there are none. Asking for the usage starts its fetching.
    fn accounts(&self, w: usize) -> Vec<Line<'static>> {
        let Some(limits) = &self.limits else { return Vec::new() };
        let usage = limits.usage();
        let mut head = vec![Span::styled("Accounts", bold())];
        if let Some(t) = usage.iter().map(|u| u.fetched_at).max().filter(|&t| t > 0) {
            head.push(Span::styled(format!("  fetched {}", clock(t)), faint()));
        }
        let mut out = vec![Line::from(head)];
        if usage.is_empty() {
            out.push(Line::styled("  no ~/.claude*/.credentials.json", faint()));
        }
        for u in &usage {
            out.extend(account_lines(u, w));
        }
        out.push(Line::raw(""));
        out
    }

    fn usage(&self, snap: &Snapshot, a: &Agent, w: usize) -> Vec<Line<'static>> {
        let root = snap.by_id.get(&a.session_id).unwrap_or(a);
        let total = snap.subtree_usage(&root.id);
        let mut out = vec![
            Line::from(vec![Span::styled(format!("Session {}", short_id(&root.id)), bold()), Span::raw(format!("  total {}", human_tokens(total.total())))]),
            Line::raw(""),
        ];
        let mut peak = 0;
        let mut by_model: BTreeMap<String, Usage> = BTreeMap::new();
        let mut members = Vec::new();
        snap.walk(&root.id, 0, &mut |x, depth| {
            peak = peak.max(x.usage.total());
            for (k, u) in &x.model_usage {
                let t = by_model.entry(k.clone()).or_default();
                *t = t.add(*u);
            }
            members.push((x, depth));
        });
        let bar_w = w.saturating_sub(30).max(4);
        for (x, depth) in members {
            let n = if peak > 0 { (bar_w as f64 * x.usage.total() as f64 / peak as f64) as usize } else { 0 };
            out.push(Line::from(vec![
                Span::raw(format!("{}  ", fit(&format!("{}{}", " ".repeat(depth), label(x)), 20))),
                Span::styled("█".repeat(n), Style::new().fg(Color::Indexed(63))),
                Span::styled("░".repeat(bar_w - n), faint()),
                Span::raw(format!(" {:>6}", human_tokens(x.usage.total()))),
            ]));
        }
        out.push(Line::raw(""));
        out.push(Line::styled("By model", bold()));
        let mut models: Vec<_> = by_model.into_iter().collect();
        models.sort_by(|a, b| b.1.total().cmp(&a.1.total()));
        for (k, u) in models {
            out.push(Line::from(vec![
                Span::raw(format!("  {} {:>7}  ", fit(&short_model(&k), 14), human_tokens(u.total()))),
                Span::styled(usage_detail(u), faint()),
            ]));
        }
        out.push(Line::raw(""));
        out.push(Line::from(vec![
            Span::styled("Session total ", bold()),
            Span::raw(format!("{}  ", human_tokens(total.total()))),
            Span::styled(usage_detail(total), faint()),
        ]));
        out
    }
}

/// One account's rows: `<name> 5h  ████░░  62%  resets 14:10` and
/// `        7d  ██░░░░  36%  resets Fri 22:00`, else `<name>  <why not>`.
pub(super) fn account_lines(u: &AccountUsage, w: usize) -> Vec<Line<'static>> {
    let name = truncate(&u.name, 16);
    let why = match (&u.error, u.fetched_at) {
        (Some(e), _) => Some(e.clone()),
        (None, 0) => Some("fetching…".into()),
        _ => None,
    };
    if let Some(why) = why {
        return vec![Line::from(vec![Span::raw(format!("{}  ", fit(&name, 20))), Span::styled(why, faint())])];
    }
    // The label column of the session bars; room on the right for "100%  resets Fri 22:00".
    let bar_w = w.saturating_sub(45).max(4);
    let pad = " ".repeat(name.chars().count());
    vec![
        limit_line(&format!("{name} 5h"), u.five_hour, bar_w, hour_minute),
        limit_line(&format!("{pad} 7d"), u.seven_day, bar_w, weekday_time),
    ]
}

/// A bar of bar_w cells filled to the limit's percent, then the percent
/// and the reset time as `when` writes it.
pub(super) fn limit_line(label: &str, l: Option<Limit>, bar_w: usize, when: fn(i64) -> String) -> Line<'static> {
    let pct = l.map_or(0.0, |l| l.pct.clamp(0.0, 100.0));
    let n = ((bar_w as f64 * pct / 100.0) as usize).min(bar_w);
    let mut spans = vec![
        Span::raw(format!("{}  ", fit(label, 20))),
        Span::styled("█".repeat(n), Style::new().fg(Color::Indexed(63))),
        Span::styled("░".repeat(bar_w - n), faint()),
        Span::raw(match l {
            Some(_) => format!(" {:>3}%", pct.round()),
            None => "    -".into(),
        }),
    ];
    if let Some(t) = l.map(|l| l.resets_at).filter(|&t| t > 0) {
        spans.push(Span::styled(format!("  resets {}", when(t)), faint()));
    }
    Line::from(spans)
}

/// Fri 22:00 in local time.
pub(super) fn weekday_time(t: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let l = local(t);
    // 1970-01-01, day 0, was a Thursday.
    let day = DAYS[(days_from_civil(l.year, l.month, l.day) + 4).rem_euclid(7) as usize];
    format!("{day} {:02}:{:02}", l.hour, l.min)
}
