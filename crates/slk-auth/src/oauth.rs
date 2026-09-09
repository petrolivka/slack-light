//! The official route in: an app you install, over Slack's documented OAuth.
//!
//! FR-A5. This exists because the session route is a grey zone (README, and
//! `docs/SLACK-ACCESS-STRATEGY.md`), and somebody whose employer would rather
//! they did not read cookies out of a browser should still be able to use
//! this client. It buys legitimacy and costs features: no `client.counts`, no
//! typing, no drafts, and realtime only through Socket Mode. The backend says
//! so in `capabilities()` rather than failing at the point of use.
//!
//! **There is no shipped client secret.** A secret in a GPL binary is not a
//! secret, and an app registered by this project would put every user's
//! access under one revocable installation. `contrib/slack-app-manifest.yml`
//! is the app; you create it in your own workspace and paste its two ids
//! here. That is more work than it should be, and it is the honest shape.

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

/// The user scopes the client actually uses.
///
/// Listed here rather than only in the manifest so the two can be compared:
/// a scope in the manifest that nothing uses is one the person granted for
/// no reason, and this client asks for as little as it can while still being
/// the thing it claims to be.
pub const SCOPES: &[&str] = &[
    "channels:history",
    "channels:read",
    "channels:write",
    "groups:history",
    "groups:read",
    "im:history",
    "im:read",
    "mpim:history",
    "mpim:read",
    "chat:write",
    "reactions:read",
    "reactions:write",
    "search:read",
    "users:read",
    "users.profile:write",
    "files:read",
    "files:write",
    "pins:read",
    "pins:write",
    "stars:read",
    "stars:write",
    "dnd:write",
    "emoji:read",
];

/// The loopback port. Fixed, because Slack matches the redirect URL against
/// the ones registered on the app and an ephemeral port would never match.
pub const REDIRECT_PORT: u16 = 41751;

pub fn redirect_uri() -> String {
    format!("http://127.0.0.1:{REDIRECT_PORT}/callback")
}

/// The two ids from the app you created. Kept beside the credentials, in a
/// file of their own, because they are configuration rather than a secret in
/// the credential sense — but the file is still 0600, because the client
/// secret is exactly as good as the app.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct App {
    pub client_id: String,
    pub client_secret: String,
}

pub fn app_path() -> PathBuf {
    slk_config::config_dir().join("oauth.toml")
}

pub fn load_app() -> Result<App> {
    let p = app_path();
    let raw = std::fs::read_to_string(&p).with_context(|| {
        format!(
            "no app registered at {}.\n\n\
             The official route needs a Slack app of your own — there is no shipped\n\
             client secret, because a secret in an open-source binary is not one.\n\n\
             1. Create an app from contrib/slack-app-manifest.yml at\n\
             \x20      https://api.slack.com/apps?new_app=1\n\
             2. Write its two ids into {}:\n\n\
             \x20   client_id = \"…\"\n\
             \x20   client_secret = \"…\"\n",
            p.display(),
            p.display()
        )
    })?;
    let app: App =
        toml::from_str(&raw).with_context(|| format!("{} is not valid TOML", p.display()))?;
    if app.client_id.is_empty() || app.client_secret.is_empty() {
        bail!("{} has an empty client_id or client_secret", p.display());
    }
    Ok(app)
}

/// What a completed install gives us.
pub struct Installed {
    pub team: String,
    pub token: String,
    pub refresh: String,
    pub expires_at: i64,
}

/// Run the browser flow and come back with a token.
///
/// The loopback listener is bound *before* the browser opens: binding after
/// would race a fast browser, and the failure — "connection refused" in a tab
/// the user is watching — looks like the client is broken rather than busy.
pub fn install(browser: Option<PathBuf>, timeout: Duration) -> Result<Installed> {
    let app = load_app()?;
    let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT)).with_context(|| {
        format!("cannot listen on 127.0.0.1:{REDIRECT_PORT} — is another sign-in running?")
    })?;
    listener.set_nonblocking(false)?;

    // Not for secrecy — the whole exchange is on loopback — but because Slack
    // will hand the code to whatever is listening on that port, and a `state`
    // that does not come back means something else answered.
    let state = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let url = format!(
        "https://slack.com/oauth/v2/authorize?client_id={}&user_scope={}&redirect_uri={}&state={state}",
        urlencode(&app.client_id),
        urlencode(&SCOPES.join(",")),
        urlencode(&redirect_uri()),
    );

    println!("Opening your browser to authorise the app.");
    println!("If nothing opens, go to:\n\n  {url}\n");
    open_browser(browser.as_deref(), &url);

    let code = wait_for_code(&listener, &state, timeout)?;
    let got = exchange(&app, &code)?;
    Ok(got)
}

/// Swap an expired access token for a fresh one.
///
/// Only meaningful when the app has token rotation turned on; without it
/// Slack returns no refresh token and the access token does not expire.
pub fn refresh(refresh_token: &str) -> Result<Installed> {
    let app = load_app()?;
    post(&[
        ("client_id", app.client_id.as_str()),
        ("client_secret", app.client_secret.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ])
}

fn exchange(app: &App, code: &str) -> Result<Installed> {
    let redirect = redirect_uri();
    post(&[
        ("client_id", app.client_id.as_str()),
        ("client_secret", app.client_secret.as_str()),
        ("code", code),
        ("redirect_uri", redirect.as_str()),
    ])
}

fn post(form: &[(&str, &str)]) -> Result<Installed> {
    // A blocking client for a one-shot exchange in a CLI command: this runs
    // before there is a runtime, and building one for a single request would
    // be the tail wagging the dog.
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let v: serde_json::Value = client
        .post("https://slack.com/api/oauth.v2.access")
        .form(form)
        .send()
        .context("asking Slack for a token")?
        .json()
        .context("Slack's answer was not JSON")?;

    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        bail!(
            "Slack refused: {}",
            v.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("no reason given")
        );
    }
    // A *user* token, not the bot's: this client reads and writes as the
    // person, and a bot token would show every message as coming from an app.
    let user = v
        .get("authed_user")
        .context("no authed_user in the answer — was `user_scope` empty?")?;
    let token = user
        .get("access_token")
        .and_then(|t| t.as_str())
        .context("no user access token in the answer")?
        .to_string();
    let refresh = user
        .get("refresh_token")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string();
    let expires_at = user
        .get("expires_in")
        .and_then(|e| e.as_i64())
        .map(|secs| now() + secs)
        .unwrap_or(0);
    let team = v
        .get("team")
        .and_then(|t| t.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("workspace")
        .to_string();
    Ok(Installed {
        team,
        token,
        refresh,
        expires_at,
    })
}

fn wait_for_code(listener: &TcpListener, state: &str, timeout: Duration) -> Result<String> {
    let deadline = std::time::Instant::now() + timeout;
    for stream in listener.incoming() {
        if std::time::Instant::now() > deadline {
            bail!("gave up waiting for the browser");
        }
        let mut stream = stream?;
        let mut line = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut line)?;
        // `GET /callback?code=…&state=… HTTP/1.1`
        let target = line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_string();
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or_default();
        let mut code = None;
        let mut got_state = None;
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("code", v)) => code = Some(urldecode(v)),
                Some(("state", v)) => got_state = Some(urldecode(v)),
                _ => {}
            }
        }
        let ok = code.is_some() && got_state.as_deref() == Some(state);
        let body = if ok {
            "<h1>Signed in</h1><p>You can close this tab and go back to the terminal.</p>"
        } else {
            "<h1>That did not work</h1><p>The terminal has the details.</p>"
        };
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.flush();
        if ok {
            return Ok(code.unwrap());
        }
        if code.is_some() {
            bail!("the answer came back with the wrong `state` — something else answered on that port");
        }
        // Anything else on that port — a favicon request, a stray probe — is
        // not the answer. Keep listening rather than failing on it.
    }
    bail!("the loopback listener closed before the browser answered")
}

fn open_browser(explicit: Option<&std::path::Path>, url: &str) {
    // The user's *own* browser here, unlike the session route's throwaway
    // profile: they are signing in to an app, which is exactly the thing a
    // browser's saved session is for.
    if let Some(p) = explicit {
        let _ = std::process::Command::new(p).arg(url).spawn();
        return;
    }
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_code_survives_percent_encoding() {
        assert_eq!(super::urldecode("a%2Fb%3Dc+d"), "a/b=c d");
        assert_eq!(super::urlencode("a/b=c d"), "a%2Fb%3Dc%20d");
        // A truncated escape is text, not a panic: this parses whatever a
        // browser sent, and CONTRIBUTING rule 2 applies to it too.
        assert_eq!(super::urldecode("%"), "%");
        assert_eq!(super::urldecode("%2"), "%2");
    }

    #[test]
    fn the_manifest_asks_for_exactly_the_scopes_the_client_uses() {
        // A scope in the manifest that nothing uses is one the person granted
        // for no reason; a scope the client uses and the manifest omits is a
        // feature that fails after they have already installed it. The two
        // lists drift the moment nothing compares them.
        let manifest = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../contrib/slack-app-manifest.yml"
        ))
        .expect("the manifest ships with the client");
        let listed: Vec<&str> = manifest
            .lines()
            .map(str::trim)
            .filter_map(|l| l.strip_prefix("- "))
            .filter(|l| l.contains(':') && !l.starts_with("http"))
            .collect();
        for want in super::SCOPES {
            assert!(listed.contains(want), "the manifest never asks for {want}");
        }
        for got in &listed {
            assert!(
                super::SCOPES.contains(got),
                "the manifest asks for {got}, which nothing uses"
            );
        }
        // And nothing administrative, ever: this is one person's client.
        assert!(!super::SCOPES.iter().any(|s| s.starts_with("admin")));
        let mut sorted = super::SCOPES.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), super::SCOPES.len(), "a scope is listed twice");
    }
}
