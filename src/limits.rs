//! The plan limits of each Claude account: how much of its 5-hour and 7-day
//! windows is used and when each resets, from Anthropic's OAuth usage
//! endpoint, the request ~/.local/bin/claude_usage.py makes.
//!
//! An account is `~/.claude` ("default") or a `~/.claude-<name>` directory
//! ("<name>") that holds `.credentials.json`. Its OAuth access token is only
//! read: never refreshed, never written, never printed. An expired one is an
//! error with no request made. The fetching runs on a thread of its own,
//! started the first time the usage is asked for, at most once a minute
//! (sooner on refresh, but never twice within 10 s).

use std::{
    cell::OnceCell,
    fmt, fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use serde_json::Value;

use crate::scan::{parse_rfc3339, unix_nanos};

const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TIMEOUT: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(60);
const MIN_GAP: Duration = Duration::from_secs(10);
pub const EXPIRED: &str = "token expired, open claude with this account";

/// One window's use: the percent used and when it resets (Unix
/// nanoseconds, 0 when the response gives none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limit {
    pub pct: f64,
    pub resets_at: i64,
}

/// What is known of one account's limits. Before its first fetch, both
/// limits are None and fetched_at is 0.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountUsage {
    pub name: String,
    pub five_hour: Option<Limit>,
    pub seven_day: Option<Limit>,
    pub fetched_at: i64,
    pub error: Option<String>, // short and fixed: never a token, a body or a library error
}

/// A config directory with credentials.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub name: String,
    pub dir: PathBuf,
}

/// `~/.claude` and every `~/.claude-*` directory holding `.credentials.json`,
/// each directory once (a symlinked copy is left out).
pub fn accounts(home: &Path) -> Vec<Account> {
    let mut extra: Vec<PathBuf> = fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".claude-"))
        .map(|e| e.path())
        .collect();
    extra.sort();
    let mut seen = std::collections::HashSet::new();
    std::iter::once(home.join(".claude"))
        .chain(extra)
        .filter(|d| d.join(".credentials.json").is_file())
        .filter(|d| seen.insert(fs::canonicalize(d).unwrap_or_else(|_| d.clone())))
        .map(|dir| {
            let base = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = match base.strip_prefix(".claude-") {
                Some(n) if !n.is_empty() => n.to_string(),
                _ if base == ".claude" => "default".to_string(),
                _ => base,
            };
            Account { name, dir }
        })
        .collect()
}

/// An OAuth access token. Debug shows no part of it and there is no
/// Display, so no format string can print it.
pub struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// The account's access token, unless it is missing or expired (as of now,
/// Unix nanoseconds). Errors are fixed texts.
fn token(dir: &Path, now: i64) -> Result<Token, String> {
    let b = fs::read(dir.join(".credentials.json")).map_err(|_| "no credentials".to_string())?;
    let v: Value = serde_json::from_slice(&b).map_err(|_| "credentials unreadable".to_string())?;
    let oauth = v.get("claudeAiOauth").unwrap_or(&Value::Null);
    let t = oauth.get("accessToken").and_then(Value::as_str).filter(|t| !t.is_empty());
    let t = t.ok_or("no claude.ai login (no OAuth token)")?;
    if oauth.get("expiresAt").and_then(Value::as_i64).is_some_and(|ms| ms <= now / 1_000_000) {
        return Err(EXPIRED.into());
    }
    Ok(Token(t.to_string()))
}

/// The 5-hour and 7-day limits in a usage response:
/// `{"five_hour": {"utilization": 62.0, "resets_at": "…"}, "seven_day": …}`,
/// utilization in percent; a window that is null or absent is None.
pub fn parse(body: &str) -> Result<(Option<Limit>, Option<Limit>), String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "unexpected response".to_string())?;
    if !v.is_object() {
        return Err("unexpected response".into());
    }
    let limit = |k: &str| -> Option<Limit> {
        let w = v.get(k)?;
        let pct = w.get("utilization")?.as_f64()?;
        let resets_at = w.get("resets_at").and_then(Value::as_str).and_then(parse_rfc3339).unwrap_or(0);
        Some(Limit { pct, resets_at })
    };
    Ok((limit("five_hour"), limit("seven_day")))
}

/// One fetch of acct's limits as of now, through get (the HTTP GET, which
/// returns the status and body or a fixed error text).
fn fetch(acct: &Account, now: i64, get: impl FnOnce(&Token) -> Result<(u16, String), String>) -> AccountUsage {
    let mut u = AccountUsage { name: acct.name.clone(), fetched_at: now, ..AccountUsage::default() };
    let got = token(&acct.dir, now).and_then(|t| get(&t)).and_then(|(status, body)| match status {
        200 => parse(&body),
        401 | 403 => Err(format!("token rejected (HTTP {status}), open claude with this account")),
        _ => Err(format!("HTTP {status}")),
    });
    match got {
        Ok((five, seven)) => (u.five_hour, u.seven_day) = (five, seven),
        Err(e) => u.error = Some(e),
    }
    u
}

/// The GET claude_usage.py makes, with its headers, in at most TIMEOUT.
fn get(token: &Token) -> Result<(u16, String), String> {
    use ureq::http::HeaderValue;
    let mut auth = HeaderValue::try_from(format!("Bearer {}", token.0)).map_err(|_| "token unreadable".to_string())?;
    auth.set_sensitive(true);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).http_status_as_error(false).build().into();
    let fail = |e: ureq::Error| match e {
        ureq::Error::Timeout(_) => "timeout".to_string(),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Io(_) => "offline".to_string(),
        _ => "request failed".to_string(),
    };
    let mut resp = agent
        .get(URL)
        .header("Authorization", auth)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "claude-code/2.0")
        .call()
        .map_err(fail)?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().map_err(fail)?;
    Ok((status, body))
}

fn now() -> i64 {
    unix_nanos(SystemTime::now())
}

/// The accounts' limits, fetched in the background once asked for.
pub struct Limits {
    accounts: Vec<Account>,
    offline: bool,
    shared: Arc<Mutex<Vec<AccountUsage>>>,
    wake: OnceCell<Sender<()>>, // set once the fetching thread runs
}

impl Limits {
    /// Limits of these accounts; offline, nothing is ever fetched and each
    /// account says "offline".
    pub fn new(accounts: Vec<Account>, offline: bool) -> Limits {
        let init = accounts.iter().map(|a| AccountUsage { name: a.name.clone(), error: offline.then(|| "offline".into()), ..AccountUsage::default() }).collect();
        Limits { accounts, offline, shared: Arc::new(Mutex::new(init)), wake: OnceCell::new() }
    }

    /// The latest of every account, in account order. The first call starts
    /// the fetching; it never waits for a fetch.
    pub fn usage(&self) -> Vec<AccountUsage> {
        if !self.offline && !self.accounts.is_empty() {
            self.wake.get_or_init(|| self.start());
        }
        self.shared.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Fetches again soon, if fetching has started.
    pub fn refresh(&self) {
        if let Some(w) = self.wake.get() {
            let _ = w.send(());
        }
    }

    fn start(&self) -> Sender<()> {
        let (wake, woken) = mpsc::channel::<()>();
        let (accounts, shared) = (self.accounts.clone(), self.shared.clone());
        thread::spawn(move || {
            loop {
                let started = Instant::now();
                for (i, a) in accounts.iter().enumerate() {
                    let u = fetch(a, now(), get);
                    if let Ok(mut g) = shared.lock()
                        && let Some(slot) = g.get_mut(i)
                    {
                        *slot = u;
                    }
                }
                match woken.recv_timeout(EVERY) {
                    Ok(()) => thread::sleep(MIN_GAP.saturating_sub(started.elapsed())),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                while woken.try_recv().is_ok() {} // refreshes asked meanwhile: this round answers them
            }
        });
        wake
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::SEC;

    const FAKE: &str = "sk-ant-oat01-FAKE-token-0123456789abcdef";

    /// A config dir under the temp dir holding credentials that expire at ms.
    fn account(tag: &str, expires_ms: i64) -> Account {
        let dir = std::env::temp_dir().join(format!("office-limits-{}-{tag}", std::process::id())).join(".claude-test");
        fs::create_dir_all(&dir).unwrap();
        let creds = format!(r#"{{"claudeAiOauth":{{"accessToken":"{FAKE}","refreshToken":"r-{FAKE}","expiresAt":{expires_ms}}}}}"#);
        fs::write(dir.join(".credentials.json"), creds).unwrap();
        Account { name: "test".into(), dir }
    }

    /// The response shape claude_usage.py gets, values made up.
    const BODY: &str = r#"{"five_hour":{"utilization":62.0,"resets_at":"2026-11-19T14:10:00.497045+00:00","limit_dollars":null},
        "seven_day":{"utilization":36.0,"resets_at":"2026-11-21T22:00:00.497070+00:00"},"seven_day_opus":null}"#;

    #[test]
    fn parses_a_usage_response() {
        let (five, seven) = parse(BODY).unwrap();
        assert_eq!(five, Some(Limit { pct: 62.0, resets_at: parse_rfc3339("2026-11-19T14:10:00.497045Z").unwrap() }));
        assert_eq!(seven.map(|l| l.pct), Some(36.0));
        assert_eq!(parse(r#"{"five_hour":null}"#), Ok((None, None)));
        assert_eq!(parse(r#"{"five_hour":{"utilization":0,"resets_at":null}}"#), Ok((Some(Limit { pct: 0.0, resets_at: 0 }), None)));
        assert!(parse("<html>").is_err() && parse("[1]").is_err());
    }

    #[test]
    fn an_expired_token_is_an_error_with_no_request() {
        let now = 2_000_000 * SEC;
        let acct = account("expired", now / 1_000_000 - 1);
        let u = fetch(&acct, now, |_| panic!("no request for an expired token"));
        assert_eq!((u.error.as_deref(), u.five_hour, u.fetched_at), (Some(EXPIRED), None, now));
        let _ = fs::remove_dir_all(acct.dir.parent().unwrap());
    }

    #[test]
    fn a_fresh_token_is_sent_and_the_answer_kept() {
        let now = 2_000_000 * SEC;
        let acct = account("fresh", now / 1_000_000 + 3_600_000);
        let u = fetch(&acct, now, |t| {
            assert_eq!(t.0, FAKE);
            Ok((200, BODY.into()))
        });
        assert_eq!((u.error, u.five_hour.map(|l| l.pct), u.seven_day.map(|l| l.pct)), (None, Some(62.0), Some(36.0)));
        let _ = fs::remove_dir_all(acct.dir.parent().unwrap());
    }

    /// Whatever the failure, the token shows in no Debug output or error,
    /// even when the server echoes it.
    #[test]
    fn the_token_never_shows() {
        let now = 2_000_000 * SEC;
        let acct = account("leak", now / 1_000_000 + 3_600_000);
        let echo = format!(r#"{{"error":"bad token {FAKE}"}}"#);
        let answers: [Result<(u16, String), String>; 5] = [Ok((401, echo.clone())), Ok((429, echo.clone())), Ok((200, echo.clone())), Ok((200, FAKE.to_string())), Err("offline".into())];
        let mut errors = Vec::new();
        for a in answers {
            let u = fetch(&acct, now, |t| {
                assert!(!format!("{t:?}").contains("FAKE"), "Debug of Token: {t:?}");
                a
            });
            let shown = format!("{u:?} {u:#?} {}", u.error.clone().unwrap_or_default());
            assert!(!shown.contains(FAKE) && !shown.contains("FAKE"), "{shown}");
            errors.push(u.error.unwrap_or_default());
        }
        assert_eq!(errors, ["token rejected (HTTP 401), open claude with this account", "HTTP 429", "", "unexpected response", "offline"]);
        let expired = fetch(&account("leak2", 1), now, |_| unreachable!());
        assert!(!format!("{expired:?}").contains("FAKE"));
        let _ = fs::remove_dir_all(acct.dir.parent().unwrap());
        let _ = fs::remove_dir_all(std::env::temp_dir().join(format!("office-limits-{}-leak2", std::process::id())));
    }

    #[test]
    fn accounts_are_config_dirs_with_credentials() {
        let home = std::env::temp_dir().join(format!("office-limits-{}-home", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        for d in [".claude", ".claude-personal", ".claude-work", ".claude-nocreds"] {
            fs::create_dir_all(home.join(d)).unwrap();
            if d != ".claude-nocreds" {
                fs::write(home.join(d).join(".credentials.json"), "{}").unwrap();
            }
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.join(".claude-work"), home.join(".claude-zlink")).unwrap();
        let names: Vec<String> = accounts(&home).into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["default", "personal", "work"]);
        let u = Limits::new(accounts(&home), true).usage();
        assert!(u.iter().all(|a| a.error.as_deref() == Some("offline")), "{u:?}");
        let _ = fs::remove_dir_all(&home);
    }
}
