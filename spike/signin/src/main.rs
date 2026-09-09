//! M0-GUI spike C: sign in with your browser.
//!
//! Launches a Chromium-family browser in a throwaway profile at Slack's
//! sign-in page, watches over the DevTools protocol for the `d` cookie and
//! the `xoxc` tokens the web client keeps in local storage, and wipes the
//! profile. Never the user's real profile; never the Slack desktop app's
//! data. The same flow msga proved, written the plain way.
//!
//!   spike-signin                       # opens the browser, waits up to 5 minutes
//!   spike-signin --timeout 20          # …or that many seconds
//!   spike-signin --browser /usr/bin/x  # a specific binary (or a nonexistent one, to test that path)
//!   spike-signin --save                # store the result the way `auth add` does — slk-dev ONLY
//!
//! Transport: `--remote-debugging-pipe`. The browser reads CDP from its fd 3
//! and writes to its fd 4, one JSON message per NUL byte. No port, no
//! websocket, no listening socket for anything else on the machine to find,
//! and it works for sandboxed browsers where the DevToolsActivePort file is
//! unreadable — msga's finding, taken as read.
//!
//! Every failure has a name a user can act on: no_browser, launch_failed,
//! no_devtools, cancelled, timeout. A silent hang is the one outcome this
//! program is not allowed to have (D9).

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Chromium-family browsers, most preferred first. Firefox is absent: it
/// has no DevTools pipe and its remote protocol is a different animal.
const BROWSERS: [&str; 6] = [
    "chromium",
    "google-chrome-stable",
    "google-chrome",
    "brave",
    "brave-browser",
    "chromium-browser",
];

/// The only workspace this spike will ever store credentials for. The `d`
/// cookie reaches every workspace on the account, so the guard is on the
/// team, and the team is the test one.
const ALLOWED_TEAM: &str = "slk-dev";

#[derive(Debug)]
enum Failure {
    NoBrowser,
    LaunchFailed(String),
    NoDevTools,
    Cancelled,
    Timeout,
}

impl Failure {
    fn code(&self) -> &'static str {
        match self {
            Failure::NoBrowser => "no_browser",
            Failure::LaunchFailed(_) => "launch_failed",
            Failure::NoDevTools => "no_devtools",
            Failure::Cancelled => "cancelled",
            Failure::Timeout => "timeout",
        }
    }
    fn advice(&self) -> String {
        match self {
            Failure::NoBrowser => "no Chromium-family browser found on PATH (chromium, google-chrome, brave). Install one, or paste the token and cookie by hand.".into(),
            Failure::LaunchFailed(e) => format!("the browser refused to start: {e}"),
            Failure::NoDevTools => "the browser started but did not answer on its DevTools pipe within 10 s. A very old or unusual browser build; paste the token and cookie by hand.".into(),
            Failure::Cancelled => "the browser window was closed before sign-in finished. Run it again when ready.".into(),
            Failure::Timeout => "sign-in did not finish in time. Run it again, or paste the token and cookie by hand.".into(),
        }
    }
}

struct Session {
    team: String,
    domain: String,
    token: String,
}

struct Outcome {
    cookie: String,
    teams: Vec<Session>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let timeout = value("--timeout")
        .and_then(|v| v.parse().ok())
        .unwrap_or(300u64);
    let save = args.iter().any(|a| a == "--save");
    let browser = value("--browser").map(PathBuf::from);

    let t0 = Instant::now();
    match run(browser, Duration::from_secs(timeout)) {
        Ok(out) => {
            println!("result=ok");
            println!("elapsed_ms={}", t0.elapsed().as_millis());
            // Never the credential itself. The prefix says which kind it is
            // and nothing else.
            println!("cookie_prefix={}", &out.cookie[..out.cookie.len().min(5)]);
            for t in &out.teams {
                println!(
                    "team={} domain={} token_prefix={}",
                    t.team,
                    t.domain,
                    &t.token[..t.token.len().min(5)]
                );
            }
            if save {
                match store(&out) {
                    Ok(n) => println!("saved={n}"),
                    Err(e) => println!("saved=0 reason={e}"),
                }
            }
        }
        Err(f) => {
            println!("result={}", f.code());
            println!("elapsed_ms={}", t0.elapsed().as_millis());
            eprintln!("{}", f.advice());
            std::process::exit(1);
        }
    }
}

fn find_browser(explicit: Option<PathBuf>) -> Result<PathBuf, Failure> {
    if let Some(p) = explicit {
        return if p.is_file() {
            Ok(p)
        } else {
            Err(Failure::NoBrowser)
        };
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for name in BROWSERS {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(name);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(Failure::NoBrowser)
}

/// A profile that will refuse `slack://` links. Without this the sign-in
/// page ends in "open in the Slack app?" and the web client never boots.
/// Chromium keeps the per-scheme decision in the profile's Preferences,
/// and this profile is brand new, so the file is ours to write.
fn seed_profile(dir: &Path) -> Result<()> {
    let default = dir.join("Default");
    std::fs::create_dir_all(&default)?;
    let prefs = json!({
        "protocol_handler": { "excluded_schemes": { "slack": true } },
        "browser": { "has_seen_welcome_page": true },
    });
    std::fs::write(default.join("Preferences"), prefs.to_string())?;
    Ok(())
}

/// The DevTools pipe: what we write goes to the browser's fd 3, what it
/// writes to its fd 4 comes back to us.
struct Cdp {
    to_browser: std::fs::File,
    from_browser: mpsc::Receiver<Value>,
    next_id: u64,
    child: Child,
}

impl Cdp {
    fn launch(browser: &Path, profile: &Path) -> Result<Self, Failure> {
        // Two pipes, made here, handed to the child as fds 3 and 4.
        let mut a = [0i32; 2];
        let mut b = [0i32; 2];
        // SAFETY: plain pipe(2); the arrays are the right size.
        if unsafe { libc::pipe(a.as_mut_ptr()) } != 0 || unsafe { libc::pipe(b.as_mut_ptr()) } != 0
        {
            return Err(Failure::LaunchFailed("pipe(2) failed".into()));
        }
        let (child_read, we_write) = (a[0], a[1]);
        let (we_read, child_write) = (b[0], b[1]);

        let mut cmd = Command::new(browser);
        cmd.arg(format!("--user-data-dir={}", profile.display()))
            .arg("--remote-debugging-pipe")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-sync")
            .arg("--disable-features=TranslateUI")
            .arg("--window-size=1000,820")
            .arg("--new-window")
            .arg("https://slack.com/signin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: only dup/dup2/close between fork and exec, on fds we own.
        //
        // The pipe ends are numbered 3..6 in the parent, so a naive
        // `dup2(child_write, 4)` lands on top of `we_write` — which the next
        // line then closes, taking the browser's fd 4 with it. That was the
        // first version, and it reported "no_devtools" in 27 ms. Both ends
        // are moved out of the way first, then placed.
        unsafe {
            cmd.pre_exec(move || {
                let r = libc::fcntl(child_read, libc::F_DUPFD, 10);
                let w = libc::fcntl(child_write, libc::F_DUPFD, 10);
                if r < 0 || w < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::dup2(r, 3) < 0 || libc::dup2(w, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for fd in [r, w, child_read, child_write, we_write, we_read] {
                    if fd > 4 {
                        libc::close(fd);
                    }
                }
                Ok(())
            });
        }
        let child = cmd
            .spawn()
            .map_err(|e| Failure::LaunchFailed(e.to_string()))?;
        // Our ends only; the child's ends are closed here so EOF means exit.
        unsafe {
            libc::close(child_read);
            libc::close(child_write);
        }
        use std::os::unix::io::FromRawFd;
        let to_browser = unsafe { std::fs::File::from_raw_fd(we_write) };
        let reader = unsafe { std::fs::File::from_raw_fd(we_read) };

        // A thread splits the stream on NUL and parses each message; the
        // main loop only ever sees whole JSON values.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(reader);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match r.read_until(0, &mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        if buf.last() == Some(&0) {
                            buf.pop();
                        }
                        if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                            if tx.send(v).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(Cdp {
            to_browser,
            from_browser: rx,
            next_id: 0,
            child,
        })
    }

    /// Send a command and wait for its reply, letting events through to
    /// `events`. Bounded, so a browser that stops answering is a named
    /// failure and not a hang.
    fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
        wait: Duration,
        events: &mut Vec<Value>,
    ) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let mut bytes = serde_json::to_vec(&msg)?;
        bytes.push(0);
        self.to_browser.write_all(&bytes)?;
        self.to_browser.flush()?;
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(anyhow!("no reply to {method} within {wait:?}"));
            }
            match self.from_browser.recv_timeout(left) {
                Ok(v) if v.get("id").and_then(Value::as_u64) == Some(id) => {
                    if let Some(e) = v.get("error") {
                        return Err(anyhow!("{method}: {e}"));
                    }
                    return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                }
                Ok(v) => events.push(v),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(anyhow!("browser closed the DevTools pipe"))
                }
            }
        }
    }

    fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

fn run(browser: Option<PathBuf>, timeout: Duration) -> Result<Outcome, Failure> {
    let browser = find_browser(browser)?;
    let profile = std::env::temp_dir().join(format!("slack-light-signin-{}", std::process::id()));
    seed_profile(&profile).map_err(|e| Failure::LaunchFailed(e.to_string()))?;

    // Whatever happens below, the profile goes: it holds the session cookie
    // the moment sign-in succeeds, and it holds a half-typed password the
    // moment it does not.
    let result = drive(&browser, &profile, timeout);
    let _ = std::fs::remove_dir_all(&profile);
    result
}

fn drive(browser: &Path, profile: &Path, timeout: Duration) -> Result<Outcome, Failure> {
    let mut cdp = Cdp::launch(browser, profile)?;
    let mut events = Vec::new();

    // The handshake: any answer at all within ten seconds. Chromium answers
    // in well under one.
    let version = cdp
        .call(
            "Browser.getVersion",
            json!({}),
            None,
            Duration::from_secs(10),
            &mut events,
        )
        .map_err(|_| Failure::NoDevTools)?;
    println!(
        "browser={}",
        version
            .get("product")
            .and_then(Value::as_str)
            .unwrap_or("?")
    );
    let _ = cdp.call(
        "Target.setDiscoverTargets",
        json!({ "discover": true }),
        None,
        Duration::from_secs(5),
        &mut events,
    );

    let started = Instant::now();
    let mut announced = false;
    loop {
        if cdp.exited() {
            return Err(Failure::Cancelled);
        }
        if started.elapsed() > timeout {
            let _ = cdp.child.kill();
            return Err(Failure::Timeout);
        }

        // Every cookie the browser holds; the `d` on slack.com is the one.
        let cookies = cdp
            .call(
                "Storage.getCookies",
                json!({}),
                None,
                Duration::from_secs(5),
                &mut events,
            )
            .map_err(|_| Failure::NoDevTools)?;
        let d = cookies
            .get("cookies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|c| {
                c.get("name").and_then(Value::as_str) == Some("d")
                    && c.get("domain")
                        .and_then(Value::as_str)
                        .is_some_and(|d| d.ends_with("slack.com"))
                    && c.get("value")
                        .and_then(Value::as_str)
                        .is_some_and(|v| v.starts_with("xoxd-"))
            })
            .and_then(|c| c.get("value").and_then(Value::as_str).map(str::to_string));

        if let Some(cookie) = d {
            if !announced {
                println!("cookie_seen_ms={}", started.elapsed().as_millis());
                announced = true;
            }
            // The tokens live in the web client's local storage, which
            // exists once it has booted. Find its page and ask.
            if let Some(teams) = read_teams(&mut cdp, &mut events) {
                let _ = cdp.child.kill();
                return Ok(Outcome { cookie, teams });
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// `localConfig_v2` on the app.slack.com origin: a map of teams, each with
/// its `xoxc` token. Present only after the web client has booted, so a
/// `None` here means "not yet", and the caller keeps waiting.
fn read_teams(cdp: &mut Cdp, events: &mut Vec<Value>) -> Option<Vec<Session>> {
    let targets = cdp
        .call(
            "Target.getTargets",
            json!({}),
            None,
            Duration::from_secs(5),
            events,
        )
        .ok()?;
    let page = targets
        .get("targetInfos")
        .and_then(Value::as_array)?
        .iter()
        .find(|t| {
            t.get("type").and_then(Value::as_str) == Some("page")
                && t.get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|u| u.starts_with("https://app.slack.com/"))
        })?
        .get("targetId")?
        .as_str()?
        .to_string();
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({ "targetId": page, "flatten": true }),
            None,
            Duration::from_secs(5),
            events,
        )
        .ok()?;
    let session = attached.get("sessionId")?.as_str()?.to_string();
    let value = cdp
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "localStorage.getItem('localConfig_v2')",
                "returnByValue": true
            }),
            Some(&session),
            Duration::from_secs(5),
            events,
        )
        .ok()?;
    let raw = value.get("result")?.get("value")?.as_str()?;
    let cfg: Value = serde_json::from_str(raw).ok()?;
    let teams = cfg
        .get("teams")?
        .as_object()?
        .values()
        .filter_map(|t| {
            let token = t.get("token")?.as_str()?;
            if !token.starts_with("xoxc-") {
                return None;
            }
            Some(Session {
                team: t.get("id")?.as_str()?.to_string(),
                domain: t.get("domain")?.as_str()?.to_string(),
                token: token.to_string(),
            })
        })
        .collect::<Vec<_>>();
    if teams.is_empty() {
        None
    } else {
        Some(teams)
    }
}

/// Store the way `auth add` stores, for the test workspace only — and
/// only after `auth.test` has said the credentials are real.
fn store(out: &Outcome) -> Result<usize> {
    let rt = tokio::runtime::Runtime::new()?;
    let mut saved = 0;
    for t in &out.teams {
        if t.domain != ALLOWED_TEAM {
            println!("skipped={} reason=not_the_test_workspace", t.domain);
            continue;
        }
        let ok = rt.block_on(auth_test(&t.domain, &t.token, &out.cookie))?;
        if !ok {
            println!("skipped={} reason=auth_test_failed", t.domain);
            continue;
        }
        let mut all = slack_light::auth::load_all().unwrap_or_default();
        all.retain(|a| a.team != t.domain);
        all.push(slack_light::auth::Account {
            team: t.domain.clone(),
            token: t.token.clone(),
            cookie: out.cookie.clone(),
        });
        slack_light::auth::save_all(&all).context("saving")?;
        saved += 1;
    }
    Ok(saved)
}

async fn auth_test(domain: &str, token: &str, cookie: &str) -> Result<bool> {
    let client = reqwest::Client::builder()
        .user_agent("slack-light/0.0 signin-spike")
        .build()?;
    let v: Value = client
        .post(format!("https://{domain}.slack.com/api/auth.test"))
        .header("Cookie", format!("d={cookie}"))
        .bearer_auth(token)
        .send()
        .await?
        .json()
        .await?;
    Ok(v.get("ok").and_then(Value::as_bool) == Some(true))
}

// Keep `Read` in scope for BufReader::read_until on older toolchains.
#[allow(dead_code)]
fn _uses_read<R: Read>(_: R) {}
