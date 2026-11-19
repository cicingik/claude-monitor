//! Markdown as the prompt panel shows a reply, close to Claude Code's own:
//! headings, lists, quotes, rules and fenced code by line; bold, italic,
//! strikethrough, inline code and links within a line; all wrapped to a
//! width with each piece keeping its style.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// A run of text in one style.
type Run = (String, Style);

fn code() -> Style {
    Style::new().fg(Color::Indexed(117))
}
fn heading() -> Style {
    Style::new().fg(Color::Indexed(212)).add_modifier(Modifier::BOLD)
}
fn faint() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// text as lines of at most w chars.
pub fn render(text: &str, w: usize) -> Vec<Line<'static>> {
    let w = w.max(1);
    let mut out = Vec::new();
    let mut fenced = false;
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            let chars: Vec<char> = line.chars().collect();
            if chars.is_empty() {
                out.push(Line::raw(""));
            }
            out.extend(chars.chunks(w).map(|c| Line::from(Span::styled(c.iter().collect::<String>(), code()))));
            continue;
        }
        if trimmed.is_empty() {
            out.push(Line::raw(""));
            continue;
        }
        if is_row(line) {
            let mut rows = vec![line];
            while i < lines.len() && is_row(lines[i]) {
                rows.push(lines[i]);
                i += 1;
            }
            out.extend(table(&rows, w));
            continue;
        }
        let indent = " ".repeat(line.len() - trimmed.len());
        if trimmed.len() >= 3 && ["-", "*", "_"].iter().any(|r| trimmed.chars().all(|c| c.to_string() == *r || c == ' ')) {
            out.push(Line::from(Span::styled("─".repeat(w), faint())));
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            out.extend(wrap(Vec::new(), String::new(), inline(trimmed[hashes..].trim(), heading()), w));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            let bar = || Span::styled("│ ", faint());
            out.extend(wrap(vec![bar()], "│ ".into(), inline(rest.trim(), faint().add_modifier(Modifier::ITALIC)), w).into_iter().enumerate().map(|(i, mut l)| {
                if i > 0 {
                    l.spans[0] = bar(); // the hanging indent of a quote is its bar
                }
                l
            }));
            continue;
        }
        let (mark, rest) = list_mark(trimmed);
        let lead = format!("{indent}{mark}");
        let hang = " ".repeat(lead.chars().count());
        out.extend(wrap(vec![Span::raw(lead)], hang, inline(rest, Style::new()), w));
    }
    out
}

/// Whether line is a table row: `| a | b |`.
fn is_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.len() > 1 && t[1..].contains('|')
}

/// A table's rows as text in aligned columns: cells padded to their
/// column's widest, ` │ ` between columns, the `|---|` row under the header
/// as a `─┼─` rule and the header bold. Too wide for w, the widest column
/// gives way first, its cells cut with `…`.
fn table(rows: &[&str], w: usize) -> Vec<Line<'static>> {
    let rule = |r: &&str| r.trim().trim_matches('|').split('|').all(|c| {
        let c = c.trim();
        !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':'))
    });
    let header = rows.len() > 1 && rule(&rows[1]);
    let cells: Vec<Option<Vec<Vec<Run>>>> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            if header && i == 1 {
                return None;
            }
            let base = if header && i == 0 { Style::new().add_modifier(Modifier::BOLD) } else { Style::new() };
            let t = r.trim();
            let t = t.strip_prefix('|').unwrap_or(t);
            let t = t.strip_suffix('|').unwrap_or(t);
            Some(t.split('|').map(|c| inline(c.trim(), base)).collect())
        })
        .collect();
    let len = |runs: &Vec<Run>| runs.iter().map(|(t, _)| t.chars().count()).sum::<usize>();
    let columns = cells.iter().flatten().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; columns];
    for row in cells.iter().flatten() {
        for (c, runs) in row.iter().enumerate() {
            widths[c] = widths[c].max(len(runs));
        }
    }
    let gaps = 3 * columns.saturating_sub(1);
    while widths.iter().sum::<usize>() + gaps > w {
        let (c, &widest) = widths.iter().enumerate().max_by_key(|(_, n)| **n).expect("a column at least");
        if widest <= 1 {
            break;
        }
        widths[c] -= 1;
    }
    let sep = || Span::styled(" │ ", faint());
    cells
        .iter()
        .map(|row| match row {
            None => Line::from(Span::styled(widths.iter().map(|n| "─".repeat(*n)).collect::<Vec<_>>().join("─┼─"), faint())),
            Some(row) => {
                let mut spans = Vec::new();
                for (c, &width) in widths.iter().enumerate() {
                    if c > 0 {
                        spans.push(sep());
                    }
                    let runs = row.get(c).cloned().unwrap_or_default();
                    let (mut used, cut) = (0, len(&runs) > width);
                    let room = if cut { width.saturating_sub(1) } else { width };
                    for (t, st) in runs {
                        let take: String = t.chars().take(room - used).collect();
                        used += take.chars().count();
                        if !take.is_empty() {
                            spans.push(Span::styled(take, st));
                        }
                    }
                    if cut && width > 0 {
                        spans.push(Span::raw("…"));
                        used += 1;
                    }
                    if c + 1 < widths.len() && width > used {
                        spans.push(Span::raw(" ".repeat(width - used)));
                    }
                }
                Line::from(spans)
            }
        })
        .collect()
}

/// A list item's marker as shown (`• ` for a bullet, `3. ` as written) and
/// the text after it; no marker for any other line.
fn list_mark(t: &str) -> (String, &str) {
    for b in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(b) {
            return ("• ".into(), rest);
        }
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") ")) {
        return (t[..digits + 2].to_string(), &t[digits + 2..]);
    }
    (String::new(), t)
}

/// The runs of one line of text: `code`, **bold** or __bold__, *italic*,
/// ~~strike~~ and [links](url) (their text, underlined), on base.
fn inline(text: &str, base: Style) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let push = |out: &mut Vec<Run>, s: &str, st: Style| match out.last_mut() {
        Some((t, last)) if *last == st => t.push_str(s),
        _ if !s.is_empty() => out.push((s.to_string(), st)),
        _ => {}
    };
    let mut rest = text;
    while !rest.is_empty() {
        // A delimiter pair: what it wraps, the style for it, and the text after.
        let span = |open: &str, close: &str| {
            let inner = rest.strip_prefix(open)?;
            let end = inner.find(close).filter(|&e| e > 0)?;
            Some((&inner[..end], &inner[end + close.len()..]))
        };
        if let Some((inner, after)) = span("``", "``").or_else(|| span("`", "`")) {
            push(&mut out, inner.strip_prefix(' ').and_then(|i| i.strip_suffix(' ')).unwrap_or(inner), code());
            rest = after;
        } else if let Some((inner, after)) = span("**", "**").or_else(|| span("__", "__")) {
            for (t, st) in inline(inner, base.add_modifier(Modifier::BOLD)) {
                push(&mut out, &t, st);
            }
            rest = after;
        } else if let Some((inner, after)) = span("~~", "~~") {
            for (t, st) in inline(inner, base.add_modifier(Modifier::CROSSED_OUT)) {
                push(&mut out, &t, st);
            }
            rest = after;
        } else if let Some((inner, after)) = span("*", "*").filter(|(i, _)| !i.starts_with(' ') && !i.ends_with(' ')) {
            for (t, st) in inline(inner, base.add_modifier(Modifier::ITALIC)) {
                push(&mut out, &t, st);
            }
            rest = after;
        } else if let Some((label, after)) = span("[", "](").and_then(|(l, a)| Some((l, &a[a.find(')')? + 1..]))) {
            for (t, st) in inline(label, base.add_modifier(Modifier::UNDERLINED)) {
                push(&mut out, &t, st);
            }
            rest = after;
        } else {
            let c = rest.chars().next().expect("rest is not empty");
            push(&mut out, &rest[..c.len_utf8()], base);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// runs word-wrapped to w after lead on the first line and hang on the
/// rest; a word longer than a line is cut. A word may change style midway
/// (`**done**.`): it stays whole.
fn wrap(lead: Vec<Span<'static>>, hang: String, runs: Vec<Run>, w: usize) -> Vec<Line<'static>> {
    let mut words: Vec<Vec<Run>> = vec![Vec::new()];
    for (text, st) in runs {
        for (i, part) in text.split(' ').enumerate() {
            if i > 0 && !words.last().expect("one at least").is_empty() {
                words.push(Vec::new());
            }
            if !part.is_empty() {
                words.last_mut().expect("one at least").push((part.to_string(), st));
            }
        }
    }
    let width = |w: &Vec<Run>| w.iter().map(|(t, _)| t.chars().count()).sum::<usize>();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut line: Vec<Span<'static>> = lead;
    let mut used: usize = line.iter().map(|s| s.content.chars().count()).sum();
    let start = used;
    let hang_w = hang.chars().count();
    for mut word in words.into_iter().filter(|w| !w.is_empty()) {
        loop {
            let ww = width(&word);
            let empty = used == start || (!out.is_empty() && used == hang_w);
            let sep = usize::from(!empty);
            if used + sep + ww <= w {
                if sep == 1 {
                    line.push(Span::raw(" "));
                }
                used += sep + ww;
                line.extend(word.drain(..).map(|(t, st)| Span::styled(t, st)));
                break;
            }
            if !empty {
                out.push(Line::from(std::mem::take(&mut line)));
                line.push(Span::raw(hang.clone()));
                used = hang_w;
                continue;
            }
            // Longer than a whole line: cut it at the line's end.
            let mut room = w.saturating_sub(used).max(1);
            let mut rest = Vec::new();
            for (t, st) in word.drain(..) {
                let n = t.chars().count();
                if room == 0 {
                    rest.push((t, st));
                } else if n <= room {
                    room -= n;
                    line.push(Span::styled(t, st));
                } else {
                    let cut: String = t.chars().take(room).collect();
                    rest.push((t.chars().skip(room).collect(), st));
                    line.push(Span::styled(cut, st));
                    room = 0;
                }
            }
            out.push(Line::from(std::mem::take(&mut line)));
            line.push(Span::raw(hang.clone()));
            used = hang_w;
            if rest.is_empty() {
                break;
            }
            word = rest;
        }
    }
    if used > hang_w || out.is_empty() {
        out.push(Line::from(line));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::to_string).collect()
    }
    /// The style of the span holding `word` in lines.
    fn style_of(lines: &[Line], word: &str) -> Style {
        lines.iter().flat_map(|l| l.spans.iter()).find(|s| s.content.contains(word)).unwrap_or_else(|| panic!("{word} in {lines:?}")).style
    }

    #[test]
    fn inline_marks_become_styles() {
        let l = render("a **bold** and *slanted* `code` ~~gone~~ [docs](https://x.y) end", 80);
        assert_eq!(text(&l), ["a bold and slanted code gone docs end"]);
        assert!(style_of(&l, "bold").add_modifier.contains(Modifier::BOLD));
        assert!(style_of(&l, "slanted").add_modifier.contains(Modifier::ITALIC));
        assert_eq!(style_of(&l, "code").fg, Some(Color::Indexed(117)));
        assert!(style_of(&l, "gone").add_modifier.contains(Modifier::CROSSED_OUT));
        assert!(style_of(&l, "docs").add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(text(&render("2 * 3 * 4 and a_b_c", 80)), ["2 * 3 * 4 and a_b_c"], "lone stars and underscores stay");
        let l = render("type `` `code` `` for code", 80);
        assert_eq!(text(&l), ["type `code` for code"]);
        assert_eq!(style_of(&l, "`code`").fg, Some(Color::Indexed(117)));
    }

    /// Wrapping keeps a style across the break, and a word whose style
    /// changes midway stays whole.
    #[test]
    fn wraps_with_styles() {
        let l = render("the **very important point** here, **done**.", 16);
        assert_eq!(text(&l), ["the very", "important point", "here, done."]);
        assert!(style_of(&l, "important").add_modifier.contains(Modifier::BOLD));
        assert!(!style_of(&l, "here").add_modifier.contains(Modifier::BOLD));
        assert_eq!(text(&render("abcdefghij", 4)), ["abcd", "efgh", "ij"]);
    }

    /// A table keeps its text in aligned columns, the header bold over a
    /// rule; too wide, the widest column is cut so the rows still line up.
    #[test]
    fn tables_align_their_columns() {
        let md = "| Key | Action |\n|-----|:------:|\n| `p` | open the **prompt** panel |\n| esc | back |";
        let l = render(md, 80);
        assert_eq!(text(&l), ["Key │ Action", format!("────┼{}", "─".repeat(22)).as_str(), "p   │ open the prompt panel", "esc │ back"]);
        assert!(style_of(&l, "Key").add_modifier.contains(Modifier::BOLD));
        assert_eq!(style_of(&l, "p").fg, Some(Color::Indexed(117)), "a cell keeps its inline style");
        assert!(style_of(&l, "prompt").add_modifier.contains(Modifier::BOLD));
        let narrow = text(&render(md, 18));
        assert_eq!(narrow, ["Key │ Action", format!("────┼{}", "─".repeat(13)).as_str(), "p   │ open the pr…", "esc │ back"]);
        assert!(narrow.iter().all(|r| r.chars().count() <= 18));
        assert_eq!(text(&render("a | b without pipes at the start", 80)), ["a | b without pipes at the start"], "not a table");
    }

    #[test]
    fn lines_by_kind() {
        let md = "# Plan\n\n- first item wraps here\n  - nested\n2. second\n> a quote that wraps\n---\n```rust\nlet x = **y**;\n```";
        let l = render(md, 14);
        assert_eq!(
            text(&l),
            ["Plan", "", "• first item", "  wraps here", "  • nested", "2. second", "│ a quote that", "│ wraps", "──────────────", "let x = **y**;"],
        );
        assert_eq!(style_of(&l, "Plan"), heading());
        assert_eq!(style_of(&l, "let x"), code(), "code is not parsed");
        assert!(style_of(&l, "quote").add_modifier.contains(Modifier::ITALIC));
    }
}
