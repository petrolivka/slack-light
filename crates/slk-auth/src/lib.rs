//! Credentials: where they live, and the two ways they arrive.
//!
//! `browser` signs in through a throwaway Chromium profile; `add` is the
//! guided paste. Both end in the same 0600 file.
//!
//! A session token and the `d` cookie are as good as the account, and the
//! cookie is account-wide: it reaches every workspace the user is signed in to,
//! not only the one named here. So they are read from a file with owner-only
//! permissions rather than from the environment, which anything running as the
//! same user can read out of `/proc`, or a command line, which is in `ps`.

pub mod browser;
pub mod oauth;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Account {
    /// The `<team>.slack.com` subdomain.
    pub team: String,
    pub token: String,
    /// The `d` cookie for a browser session. Empty for an OAuth install,
    /// which is how the two are told apart — a session token without its
    /// cookie is useless, so there is no ambiguous case.
    #[serde(default)]
    pub cookie: String,
    /// Set when the workspace's app has token rotation turned on. Empty
    /// otherwise, and Slack then never expires the token.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub refresh: String,
    /// Unix time the access token stops working, or zero for never.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub expires_at: i64,
    /// `xapp-…`. Socket Mode is the only realtime the official route has, and
    /// it needs an app-level token that OAuth does not hand out — the person
    /// copies it from the app's own settings page. Without one the backend
    /// says `realtime: false` rather than pretending.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_token: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Account {
    /// Which of the two routes this account is. See `SLACK-ACCESS-STRATEGY`.
    pub fn is_oauth(&self) -> bool {
        self.cookie.is_empty()
    }
}

pub fn path() -> PathBuf {
    slk_config::config_dir().join("auth.json")
}

/// Either shape the file may be in.
///
/// It began as one account and grew a list. Both are read, because a file
/// somebody wrote by hand a week ago should not stop working.
#[derive(Deserialize)]
#[serde(untagged)]
enum Stored {
    One(Account),
    Many { accounts: Vec<Account> },
}

/// Every configured workspace, in the order they were added.
pub fn load_all() -> Result<Vec<Account>> {
    let p = path();
    let raw = std::fs::read_to_string(&p).with_context(|| {
        format!(
            "no credentials at {}.\n\nRun `slack-light auth add` to set them up.",
            p.display()
        )
    })?;

    #[cfg(unix)]
    tighten(&p)?;

    let stored: Stored =
        serde_json::from_str(&raw).with_context(|| format!("{} is not valid JSON", p.display()))?;
    let mut accounts = match stored {
        Stored::One(a) => vec![a],
        Stored::Many { accounts } => accounts,
    };
    for a in accounts.iter_mut() {
        a.cookie = a.cookie.trim_start_matches("d=").to_string();
    }
    accounts.retain(|a| !a.team.is_empty() && !a.token.is_empty() && !a.cookie.is_empty());
    if accounts.is_empty() {
        bail!(
            "{}: no usable account; each needs team, token and cookie",
            p.display()
        );
    }
    Ok(accounts)
}

#[cfg(unix)]
fn tighten(p: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(p)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))?;
        eprintln!("note: tightened {} from {mode:o} to 600", p.display());
    }
    Ok(())
}

pub fn load() -> Result<Account> {
    let p = path();
    let raw = std::fs::read_to_string(&p).with_context(|| {
        format!(
            "no credentials at {}.\n\nRun `slack-light auth add` to set them up.",
            p.display()
        )
    })?;

    // A credential file the group or the world can read is a finding, not a
    // warning to print and move past.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&p)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
            eprintln!("note: tightened {} from {mode:o} to 600", p.display());
        }
    }

    let mut a: Account =
        serde_json::from_str(&raw).with_context(|| format!("{} is not valid JSON", p.display()))?;
    a.cookie = a.cookie.trim_start_matches("d=").to_string();
    if a.team.is_empty() || a.token.is_empty() || a.cookie.is_empty() {
        bail!("{}: team, token and cookie must all be set", p.display());
    }
    Ok(a)
}

/// Add one workspace, keeping the others.
pub fn add_account(a: Account) -> Result<PathBuf> {
    let mut all = load_all().unwrap_or_default();
    // Re-adding a workspace replaces its credentials rather than making a
    // second entry, which is what someone whose session expired is doing.
    all.retain(|x| x.team != a.team);
    all.push(a);
    save_all(&all)
}

pub fn save_all(accounts: &[Account]) -> Result<PathBuf> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let body = serde_json::json!({ "accounts": accounts });
    std::fs::write(&p, serde_json::to_string_pretty(&body)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(p)
}

/// The guided flow. Deliberately explicit about what is being handed over,
/// because the honest answer is "a key to your whole Slack account".
pub fn add() -> Result<()> {
    use std::io::{self, Write};

    println!(
        "\
slack-light — sign in

This uses your existing browser session rather than an app you install, which
is the only route that reaches unread counts, typing indicators and presence.
It is unofficial and undocumented: read the warning in the README first.

The cookie below is account-wide. It reaches every workspace you are signed in
to, not only the one you name here. It is stored at {} with permissions 0600
and never leaves this machine.

Open the workspace in your browser, then open developer tools on that tab.

1. The token. In the Console:

     Object.values(JSON.parse(localStorage.localConfig_v2).teams)
           .map(t => ({{ domain: t.domain, token: t.token }}))

   If localConfig_v2 is not there, use Network instead: filter on /api/, click
   any request, and read the `token` field from its payload.

2. The cookie. It is HttpOnly, so the console cannot see it.
   Application > Storage > Cookies > https://slack.com > the row named `d`.
",
        path().display()
    );

    let ask = |prompt: &str| -> Result<String> {
        print!("{prompt}");
        io::stdout().flush()?;
        let mut s = String::new();
        io::stdin().read_line(&mut s)?;
        Ok(s.trim().to_string())
    };

    let team = ask("workspace subdomain (the part before .slack.com): ")?;
    let token = ask("token (xoxc-…): ")?;
    let cookie = ask("cookie d (xoxd-…): ")?;

    if !token.starts_with("xoxc-") {
        eprintln!("warning: that does not look like a session token (expected xoxc-…)");
    }

    let account = Account {
        team,
        token,
        cookie: cookie.trim_start_matches("d=").to_string(),
        ..Default::default()
    };
    let p = add_account(account)?;
    let n = load_all().map(|a| a.len()).unwrap_or(1);
    println!(
        "\nsaved to {} (0600); {n} workspace{} configured. Run `slack-light` to start.",
        p.display(),
        if n == 1 { "" } else { "s" }
    );
    Ok(())
}
