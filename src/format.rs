//! Text as the monitor prints it:
//! token counts, model names, ages, Go's %q quoting, local times, durations
//! and the --dump report.

use std::{
    fmt::Write as _,
    io::{self, Write},
};

use crate::scan::{Agent, Kind, SEC, Snapshot, Status, Usage, truncate};

/// 1234 as 1.2k and 1234567 as 1.2M.
pub fn human_tokens(n: i64) -> String {
    let f = n as f64;
    match n {
        ..1_000 => format!("{n}"),
        1_000..10_000 => format!("{:.1}k", f / 1e3),
        10_000..1_000_000 => format!("{}k", n / 1_000),
        1_000_000..10_000_000 => format!("{:.1}M", f / 1e6),
        10_000_000..1_000_000_000 => format!("{}M", n / 1_000_000),
        _ => format!("{:.1}B", f / 1e9),
    }
}

/// claude-sonnet-4-5-20250929 as sonnet-4-5.
pub fn short_model(m: &str) -> String {
    if m.is_empty() {
        return "-".into();
    }
    let m = m.strip_prefix("claude-").unwrap_or(m);
    let b = m.as_bytes();
    let n = b.len();
    if n >= 9 && b[n - 9] == b'-' && b[n - 8..].iter().all(u8::is_ascii_digit) { m[..n - 9].into() } else { m.into() }
}

/// The time since t as 5s, 3m, 2h or 4d; "-" for never.
pub fn age(now: i64, t: i64) -> String {
    if t == 0 {
        return "-".into();
    }
    let d = now - t;
    match d {
        _ if d < 60 * SEC => format!("{}s", (d / SEC).max(0)),
        _ if d < 3600 * SEC => format!("{}m", d / (60 * SEC)),
        _ if d < 86_400 * SEC => format!("{}h", d / (3600 * SEC)),
        _ => format!("{}d", d / (86_400 * SEC)),
    }
}

/// The first 8 characters of an id.
pub fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// The display name of an agent: its description for subagents, the short
/// id and AI title for sessions.
pub fn label(a: &Agent) -> String {
    match (a.kind, a.title.is_empty()) {
        (Kind::Session, true) => format!("{} ({})", short_id(&a.id), a.project),
        (Kind::Session, false) => format!("{} {}", short_id(&a.id), a.title),
        (Kind::Subagent, true) => a.id.clone(),
        (Kind::Subagent, false) => a.title.clone(),
    }
}

pub fn agent_type(a: &Agent) -> &str {
    match a.kind {
        Kind::Session => "session",
        Kind::Subagent if a.agent_type.is_empty() => "agent",
        Kind::Subagent => &a.agent_type,
    }
}

pub fn usage_detail(u: Usage) -> String {
    format!(
        "in {} · out {} · cache r {} w {}",
        human_tokens(u.input),
        human_tokens(u.output),
        human_tokens(u.cache_read),
        human_tokens(u.cache_write)
    )
}

/// Pads or cuts s to exactly w characters.
pub fn fit(s: &str, w: usize) -> String {
    let s = truncate(s, w);
    let n = s.chars().count();
    format!("{s}{}", " ".repeat(w.saturating_sub(n)))
}

/// Go's Duration.String: 1h30m0s, 2m0s, 45s, 1.5s, 300ms.
pub fn go_duration(d: i64) -> String {
    if d == 0 {
        return "0s".into();
    }
    let u = d.unsigned_abs();
    let frac = |v: u64, prec: u32| -> (String, u64) {
        let p = 10u64.pow(prec);
        match v % p {
            0 => (String::new(), v / p),
            f => (format!(".{}", format!("{f:0w$}", w = prec as usize).trim_end_matches('0')), v / p),
        }
    };
    let s = if u < SEC as u64 {
        let (unit, prec) = match u {
            ..1_000 => ("ns", 0),
            1_000..1_000_000 => ("µs", 3),
            _ => ("ms", 6),
        };
        let (f, i) = frac(u, prec);
        format!("{i}{f}{unit}")
    } else {
        let (f, i) = frac(u, 9);
        let mut s = format!("{}{f}s", i % 60);
        let m = i / 60;
        if m > 0 {
            s = format!("{}m{s}", m % 60);
            if m / 60 > 0 {
                s = format!("{}h{s}", m / 60);
            }
        }
        s
    };
    if d < 0 { format!("-{s}") } else { s }
}

/// 24h0m0s as 24h and 1h30m0s as 1h30m.
pub fn short_duration(d: i64) -> String {
    let mut s = go_duration(d);
    if s.ends_with("m0s") {
        s.truncate(s.len() - 2);
    }
    if s.ends_with("h0m") {
        s.truncate(s.len() - 2);
    }
    s
}

/// Go's time.ParseDuration: "24h", "1h30m", "90s", "1.5h", "0".
pub fn parse_duration(s: &str) -> Result<i64, String> {
    let bad = || format!("invalid duration {s:?}");
    let (neg, mut rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if rest == "0" {
        return Ok(0);
    }
    if rest.is_empty() {
        return Err(bad());
    }
    let mut total: i64 = 0;
    while !rest.is_empty() {
        let int_len = rest.bytes().take_while(u8::is_ascii_digit).count();
        let int: i64 = if int_len > 0 { rest[..int_len].parse().map_err(|_| bad())? } else { 0 };
        rest = &rest[int_len..];
        let (mut frac, mut scale, mut frac_len) = (0i64, 1i64, 0);
        if let Some(r) = rest.strip_prefix('.') {
            frac_len = r.bytes().take_while(u8::is_ascii_digit).count();
            for c in r[..frac_len].bytes().take(18) {
                frac = frac * 10 + i64::from(c - b'0');
                scale *= 10;
            }
            rest = &r[frac_len..];
            frac_len += 1;
        }
        if int_len == 0 && frac_len <= 1 {
            return Err(bad());
        }
        let unit_len = rest.find(|c: char| c == '.' || c.is_ascii_digit()).unwrap_or(rest.len());
        let unit = match &rest[..unit_len] {
            "ns" => 1,
            "us" | "µs" | "μs" => 1_000,
            "ms" => 1_000_000,
            "s" => SEC,
            "m" => 60 * SEC,
            "h" => 3600 * SEC,
            "" => return Err(format!("missing unit in duration {s:?}")),
            u => return Err(format!("unknown unit {u:?} in duration {s:?}")),
        };
        rest = &rest[unit_len..];
        let v = int.checked_mul(unit).ok_or_else(bad)?;
        total = total.checked_add(v + (frac as f64 * unit as f64 / scale as f64) as i64).ok_or_else(bad)?;
    }
    Ok(if neg { -total } else { total })
}

/// Go's %q of a string (strconv.Quote).
pub fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\x07' => o.push_str("\\a"),
            '\x08' => o.push_str("\\b"),
            '\x0c' => o.push_str("\\f"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\x0b' => o.push_str("\\v"),
            c if c < ' ' || c == '\x7f' => _ = write!(o, "\\x{:02x}", c as u32),
            c if printable(c) => o.push(c),
            c if (c as u32) < 0x10000 => _ = write!(o, "\\u{:04x}", c as u32),
            c => _ = write!(o, "\\U{:08x}", c as u32),
        }
    }
    o.push('"');
    o
}

/// Go's unicode.IsPrint, near enough: no controls, no spaces but ' ',
/// no format or private-use characters.
fn printable(c: char) -> bool {
    c == ' '
        || !(c.is_control()
            || c.is_whitespace()
            || matches!(c, '\u{ad}' | '\u{600}'..='\u{605}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{e000}'..='\u{f8ff}'))
}

/// A moment in the local time zone.
pub struct Local {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub min: i64,
    pub sec: i64,
    pub offset: i64, // seconds east of UTC
}

/// Unix nanoseconds t in the local time zone.
pub fn local(t: i64) -> Local {
    let secs = t.div_euclid(SEC) as libc::time_t;
    // SAFETY: localtime_r and localtime_s only write the tm they are given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    #[cfg(unix)]
    let ok = !unsafe { libc::localtime_r(&secs, &mut tm) }.is_null();
    #[cfg(windows)]
    let ok = unsafe { libc::localtime_s(&mut tm, &secs) } == 0;
    if !ok {
        tm = unsafe { std::mem::zeroed() };
    }
    let mut l = Local {
        year: i64::from(tm.tm_year) + 1900,
        month: i64::from(tm.tm_mon) + 1,
        day: i64::from(tm.tm_mday),
        hour: i64::from(tm.tm_hour),
        min: i64::from(tm.tm_min),
        sec: i64::from(tm.tm_sec),
        offset: 0,
    };
    #[cfg(unix)]
    {
        l.offset = tm.tm_gmtoff;
    }
    // Windows' tm has no tm_gmtoff: the offset is the local wall clock minus UTC.
    #[cfg(windows)]
    if ok {
        let wall = crate::scan::days_from_civil(l.year, l.month, l.day) * 86_400 + l.hour * 3600 + l.min * 60 + l.sec;
        l.offset = wall - secs;
    }
    l
}

/// 15:04:05 in local time.
pub fn clock(t: i64) -> String {
    let l = local(t);
    format!("{:02}:{:02}:{:02}", l.hour, l.min, l.sec)
}

/// 15:04 in local time.
pub fn hour_minute(t: i64) -> String {
    let l = local(t);
    format!("{:02}:{:02}", l.hour, l.min)
}

/// 2006-01-02 15:04:05 in local time; Go's zero time for never.
pub fn datetime(t: i64) -> String {
    if t == 0 {
        return "0001-01-01 00:00:00".into();
    }
    let l = local(t);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", l.year, l.month, l.day, l.hour, l.min, l.sec)
}

/// RFC 3339 in local time, seconds only: 2006-01-02T15:04:05+07:00.
pub fn rfc3339(t: i64) -> String {
    let l = local(t);
    let zone = match l.offset {
        0 => "Z".to_string(),
        o => format!("{}{:02}:{:02}", if o < 0 { '-' } else { '+' }, o.abs() / 3600, o.abs() % 3600 / 60),
    };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{zone}", l.year, l.month, l.day, l.hour, l.min, l.sec)
}

/// Writes snap as plain text: the agent tree with status, model and exact
/// token counts, then every edge, then usage by model. The Go --dump format.
pub fn dump(w: &mut impl Write, snap: &Snapshot, roots: &str, since: i64) -> io::Result<()> {
    let running = snap.by_id.values().filter(|a| a.status == Status::Running).count();
    let window = if since > 0 { short_duration(since) } else { "all".into() };
    writeln!(w, "claude-monitor dump  root={roots}  since={window}  scanned={}", rfc3339(snap.scanned_at))?;
    write!(w, "sessions={} agents={} running={}\n\nTREE\n", snap.roots.len(), snap.by_id.len(), running)?;
    for r in &snap.roots {
        let mut lines = Vec::new();
        snap.walk(r, 0, &mut |a, depth| {
            let u = a.usage;
            lines.push(format!(
                "{}{} {}  {}  {}  model={}  turns={}  tokens={} (in={} out={} cache_read={} cache_write={})  age={}  {}",
                "  ".repeat(depth),
                a.status.glyph(),
                a.id,
                agent_type(a),
                a.status.name(),
                short_model(&a.model),
                a.turns,
                human_tokens(u.total()),
                u.input,
                u.output,
                u.cache_read,
                u.cache_write,
                age(snap.scanned_at, a.last_activity),
                quote(&a.title)
            ));
        });
        for l in lines {
            writeln!(w, "{l}")?;
        }
        if !snap.by_id[r].children.is_empty() {
            let t = snap.subtree_usage(r);
            writeln!(w, "  = session {} with subagents: tokens={} (out={})", short_id(r), human_tokens(t.total()), t.output)?;
        }
    }

    write!(w, "\nEDGES ({})\n", snap.edges.len())?;
    for e in &snap.edges {
        writeln!(w, "{:<8} {} -> {}  {}  {}", e.kind.name(), e.from, e.to, datetime(e.at), quote(&e.label))?;
    }

    writeln!(w, "\nUSAGE BY MODEL")?;
    let mut total = Usage::default();
    for (k, u) in &snap.by_model {
        total = total.add(*u);
        writeln!(w, "{k:<28} tokens={:<6} in={} out={} cache_read={} cache_write={}", human_tokens(u.total()), u.input, u.output, u.cache_read, u.cache_write)?;
    }
    writeln!(
        w,
        "{:<28} tokens={:<6} in={} out={} cache_read={} cache_write={}",
        "TOTAL",
        human_tokens(total.total()),
        total.input,
        total.output,
        total.cache_read,
        total.cache_write
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_names_print_like_go() {
        let cases = [(999, "999"), (1234, "1.2k"), (12_345, "12k"), (1_234_567, "1.2M"), (12_345_678, "12M"), (1_500_000_000, "1.5B")];
        for (n, want) in cases {
            assert_eq!(human_tokens(n), want);
        }
        assert_eq!(short_model("claude-sonnet-4-5-20250929"), "sonnet-4-5");
        assert_eq!(short_model("claude-opus-4-5"), "opus-4-5");
        assert_eq!(short_model(""), "-");
        assert_eq!(age(100 * SEC, 0), "-");
        assert_eq!(age(100 * SEC, 99 * SEC), "1s");
        assert_eq!(age(SEC, 5 * SEC), "0s");
        assert_eq!(age(7200 * SEC, SEC), "1h");
        assert_eq!(quote("a\"b\\c\n\u{1b}é"), r#""a\"b\\c\n\x1bé""#);
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("ab", 4), "ab  ");
    }

    #[test]
    fn durations_parse_and_print_like_go() {
        for (s, d) in [("24h", 86_400 * SEC), ("1h30m", 5400 * SEC), ("90s", 90 * SEC), ("1.5h", 5400 * SEC), ("0", 0), ("300ms", 300_000_000)] {
            assert_eq!(parse_duration(s), Ok(d), "{s}");
        }
        assert!(parse_duration("24").is_err());
        assert!(parse_duration("h").is_err());
        assert_eq!(go_duration(86_400 * SEC), "24h0m0s");
        assert_eq!(go_duration(90 * SEC), "1m30s");
        assert_eq!(go_duration(1_500_000_000), "1.5s");
        assert_eq!(go_duration(300_000_000), "300ms");
        assert_eq!(short_duration(86_400 * SEC), "24h");
        assert_eq!(short_duration(5400 * SEC), "1h30m");
        assert_eq!(short_duration(120 * SEC), "2m");
    }

    /// Windows has no tm_gmtoff and takes the offset as wall clock minus UTC.
    /// Where tm_gmtoff exists, the two agree.
    #[test]
    fn offset_is_wall_clock_minus_utc() {
        for secs in [0, 1_700_000_000, 1_720_000_000, 1_735_689_599] {
            let l = local(secs * SEC);
            let wall = crate::scan::days_from_civil(l.year, l.month, l.day) * 86_400 + l.hour * 3600 + l.min * 60 + l.sec;
            assert_eq!(wall - secs, l.offset, "{secs}");
        }
    }
}
