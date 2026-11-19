//! Text and time helpers: tool summaries, cut lines, and RFC 3339 times.

use serde_json::Value;

use super::SEC;

/// The string at key in v, or "" when it is missing or not a string.
pub(super) fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// How much of the running tool call the card's "now:" may show (3 lines).
pub(super) const NOW_MAX: usize = 150;

/// "Tool: arg" from a tool_use input, the arg's lines joined and cut to max.
/// What a call of tool asks the user, as lines: AskUserQuestion's questions
/// with their numbered options, ExitPlanMode's plan; "" for other tools.
pub(super) fn asking(tool: &str, input: &Value) -> String {
    match tool {
        "AskUserQuestion" => {
            let mut out = Vec::new();
            for q in input.get("questions").and_then(Value::as_array).into_iter().flatten() {
                out.push(s(q, "question").to_string());
                for (i, o) in q.get("options").and_then(Value::as_array).into_iter().flatten().enumerate() {
                    let d = s(o, "description");
                    out.push(if d.is_empty() { format!("  {}. {}", i + 1, s(o, "label")) } else { format!("  {}. {} — {d}", i + 1, s(o, "label")) });
                }
            }
            out.join("\n")
        }
        "ExitPlanMode" => s(input, "plan").trim().to_string(),
        _ => String::new(),
    }
}

pub(crate) fn tool_summary(name: &str, input: &Value, max: usize) -> String {
    let arg = match name {
        "AskUserQuestion" => {
            let qs: Vec<&str> = input.get("questions").and_then(Value::as_array).into_iter().flatten().map(|q| s(q, "question")).collect();
            qs.join(" · ")
        }
        "ExitPlanMode" => "approve the plan".to_string(),
        "Bash" => s(input, "command").to_string(),
        "Read" | "Write" | "Edit" | "NotebookEdit" => short_path(s(input, "file_path")),
        "Agent" | "Task" => {
            let (d, t) = (s(input, "description"), s(input, "subagent_type"));
            if t.is_empty() { d.to_string() } else { format!("{d} ({t})") }
        }
        "SendMessage" => format!("to {}", s(input, "to")),
        _ => ["description", "file_path", "path", "pattern", "query", "skill", "url", "name"]
            .iter()
            .map(|k| s(input, k))
            .find(|v| !v.is_empty())
            .unwrap_or("")
            .to_string(),
    };
    let arg = truncate(&arg.split_whitespace().collect::<Vec<_>>().join(" "), max);
    if arg.is_empty() { name.to_string() } else { format!("{name}: {arg}") }
}

/// Keeps the last two path segments.
pub(super) fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.trim_end_matches('/').split('/').collect();
    if parts.len() <= 3 {
        return p.to_string();
    }
    format!("…/{}", parts[parts.len() - 2..].join("/"))
}

/// The first non-empty line of s, trimmed and cut to max characters.
pub fn one_line(s: &str, max: usize) -> String {
    s.split('\n').map(str::trim).find(|l| !l.is_empty()).map_or_else(String::new, |l| truncate(l, max))
}

/// Cuts s to at most max characters, marking the cut with "…".
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return s.chars().take(max).collect();
    }
    s.chars().take(max - 1).chain(['…']).collect()
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix nanoseconds of an RFC 3339 time such as 2026-01-05T10:20:30.123Z.
pub fn parse_rfc3339(t: &str) -> Option<i64> {
    let b = t.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let d = b.get(r)?;
        d.iter().all(u8::is_ascii_digit).then(|| d.iter().fold(0, |n, c| n * 10 + i64::from(c - b'0')))
    };
    let sep = |i: usize, c: u8| b.get(i) == Some(&c);
    if !(sep(4, b'-') && sep(7, b'-') && sep(10, b'T') && sep(13, b':') && sep(16, b':')) {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let mut i = 19;
    let mut nanos = 0;
    if sep(i, b'.') {
        let n = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return None;
        }
        for k in 0..9 {
            nanos = nanos * 10 + if k < n { i64::from(b[i + 1 + k] - b'0') } else { 0 };
        }
        i += 1 + n;
    }
    let offset = match b.get(i) {
        Some(b'Z') if i + 1 == b.len() => 0,
        Some(&c @ (b'+' | b'-')) if i + 6 == b.len() && sep(i + 3, b':') => {
            let o = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
            if c == b'-' { -o } else { o }
        }
        _ => return None,
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset;
    Some(secs * SEC + nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_parses_zulu_offsets_and_fractions() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:01Z"), Some(SEC));
        assert_eq!(parse_rfc3339("1970-01-01T07:00:00+07:00"), Some(0));
        assert_eq!(parse_rfc3339("2026-01-05T10:20:30.5Z"), Some((days_from_civil(2026, 1, 5) * 86_400 + 37_230) * SEC + SEC / 2));
        assert_eq!(parse_rfc3339("2026-01-05 10:20:30Z"), None);
        assert_eq!(parse_rfc3339("2026-13-05T10:20:30Z"), None);
    }
}
