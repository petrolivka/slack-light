//! Slack's web API, over either of the two ways in.
//!
//! **Session** is an `xoxc` token and the `d` cookie read out of a browser.
//! It is the route the client is built on, because it is the only one that
//! reaches the endpoints parity depends on — `client.counts` for every unread
//! badge in a single call, `drafts.*`, `saved.list`, and a websocket that
//! carries typing and presence. M0 measured all of it working. It is also
//! undocumented, so everything here assumes Slack will change it: defensive
//! parsing, shapes pinned by fixtures rather than trusted.
//!
//! **OAuth** is an `xoxp` user token from an app the person installed
//! themselves (FR-A5). Same host, same methods, same parsing — the difference
//! is the header, a handful of endpoints that are not public, and Socket Mode
//! instead of the client websocket. One module rather than two, because two
//! implementations of `conversations.history` is how one of them stops
//! matching the fixtures.
//!
//! What OAuth cannot do it says in `capabilities()`, so the interface
//! disables the action rather than offering something that will fail.

use crate::backend::*;
use crate::error::{ErrorKind, Result, SlackError};
use crate::events::{self, EventStream, RtEvent};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use slk_core::{
    Bookmark, ChannelId, Conversation, FileId, Message, SectionKind, SidebarSection, TeamId, Ts,
    User, UserId, Workspace,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

/// Sent on every request. Honest about what this is, which is the only
/// defensible position for an unofficial client and also what a rate limiter
/// wants to see.
const USER_AGENT: &str = concat!(
    "slack-light/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/petrolivka/slack-light)"
);

/// Which of the two ways in this is. See the module comment, and
/// `docs/SLACK-ACCESS-STRATEGY.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// A browser session: `xoxc` plus the `d` cookie.
    Session,
    /// An app the user installed: `xoxp`, and only the documented methods.
    OAuth,
}

#[derive(Clone)]
pub struct Credentials {
    /// The `<team>.slack.com` subdomain.
    pub domain: String,
    pub token: String,
    /// The value of the `d` cookie, without the `d=`. Empty for OAuth, which
    /// is what `Route::of` reads.
    pub cookie: String,
    /// `xapp-…`, for Socket Mode. Only OAuth has one, and only if the person
    /// pasted it: OAuth does not hand app-level tokens out.
    pub app_token: String,
}

impl Credentials {
    pub fn route(&self) -> Route {
        // A session token without its cookie is useless, so there is no
        // ambiguous case to get wrong.
        if self.cookie.is_empty() {
            Route::OAuth
        } else {
            Route::Session
        }
    }
}

pub struct WebBackend {
    http: reqwest::Client,
    creds: Credentials,
    base: String,
    team: TeamId,
    self_id: UserId,
    /// The freshest `reconnect_url` the server pushed. M0 found Slack sends
    /// these unprompted; using one is cheaper and more correct than
    /// re-handshaking.
    reconnect_url: Arc<Mutex<Option<String>>>,
    /// A crude gate that keeps us under Slack's per-method tiers without
    /// modelling them: at most this many requests in flight at once.
    gate: Arc<tokio::sync::Semaphore>,
    /// When Slack's `Retry-After` expires. Every request waits for it, so one
    /// 429 slows the whole client down rather than each call discovering the
    /// limit for itself — which is how a rate limit becomes a retry storm.
    paused_until: Arc<Mutex<Option<std::time::Instant>>>,
    /// The way in to the live socket, for the few things that are sent rather
    /// than requested. `None` until `connect`, and replaced on every
    /// reconnect, so a frame written to a dead socket goes nowhere instead of
    /// waking a task that has already returned.
    outbound: Arc<Mutex<Option<mpsc::Sender<String>>>>,
    /// Slack's frames are numbered per connection; so are ours.
    frame_id: Arc<std::sync::atomic::AtomicI64>,
    /// Whether this is an Enterprise Grid org session, in which case every
    /// call carries `team_id` — see `whoami`.
    grid: Arc<Mutex<bool>>,
}

impl WebBackend {
    /// Connect and identify. Fails with `ErrorKind::Auth` when the credentials
    /// are stale, which is the case the UI must handle without losing state.
    pub async fn connect(creds: Credentials) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .gzip(true)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| SlackError::new("client", ErrorKind::Transport, e.to_string()))?;

        let base = format!("https://{}.slack.com/api", creds.domain);
        let mut me = WebBackend {
            http,
            creds,
            base,
            team: TeamId::new(""),
            self_id: UserId::new(""),
            reconnect_url: Arc::new(Mutex::new(None)),
            gate: Arc::new(tokio::sync::Semaphore::new(4)),
            paused_until: Arc::new(Mutex::new(None)),
            outbound: Arc::new(Mutex::new(None)),
            frame_id: Arc::new(std::sync::atomic::AtomicI64::new(1000)),
            grid: Arc::new(Mutex::new(false)),
        };
        let ws = me.whoami().await?;
        me.team = ws.id.clone();
        me.self_id = ws.self_id.clone();
        Ok(me)
    }

    /// The OAuth route's boot: what an installed app is actually allowed.
    ///
    /// `client.userBoot` gives the session route conversations, users, prefs
    /// and the muted list in one call. There is no public equivalent, so this
    /// is `users.conversations` and nothing else — no muted list (that is a
    /// pref, and prefs are not readable), no notification settings, no users
    /// (the engine resolves names one at a time as it needs them, which it
    /// already does for the session route's unknown ids).
    ///
    /// The result is a client that boots a little emptier and fills in as it
    /// goes, rather than one that pretends to know things it cannot ask.
    async fn boot_oauth(&self) -> Result<Boot> {
        let mut conversations = Vec::new();
        let mut cursor = String::new();
        // Paged, because a person in three hundred channels is not unusual and
        // Slack's default page is a hundred.
        for _ in 0..10 {
            let mut form: Vec<(&str, &str)> = vec![
                ("types", "public_channel,private_channel,im,mpim"),
                ("exclude_archived", "true"),
                ("limit", "200"),
            ];
            if !cursor.is_empty() {
                form.push(("cursor", &cursor));
            }
            let v = self.call("users.conversations", &form).await?;
            if let Some(a) = v.get("channels").and_then(Value::as_array) {
                conversations.extend(a.iter().filter_map(|c| Conversation::parse(&self.team, c)));
            }
            cursor = v
                .get("response_metadata")
                .and_then(|m| m.get("next_cursor"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if cursor.is_empty() {
                break;
            }
        }
        Ok(Boot {
            conversations,
            users: Vec::new(),
            muted: Vec::new(),
            notify: Vec::new(),
        })
    }

    /// A call authenticated with the *app-level* token rather than the user's.
    ///
    /// Exactly one method needs this — `apps.connections.open` — and it
    /// refuses a user token, so it cannot go through `call`.
    async fn app_call(&self, method: &str) -> Result<Value> {
        let res = self
            .http
            .post(format!("{}/{method}", self.base))
            .bearer_auth(&self.creds.app_token)
            .send()
            .await
            .map_err(|e| SlackError::new(method, ErrorKind::Transport, e.to_string()))?;
        let v: Value = res
            .json()
            .await
            .map_err(|e| SlackError::new(method, ErrorKind::Shape, e.to_string()))?;
        if v.get("ok").and_then(Value::as_bool) != Some(true) {
            let detail = v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("no reason given");
            return Err(SlackError::new(
                method,
                if detail.contains("auth") {
                    ErrorKind::Auth
                } else {
                    ErrorKind::Other
                },
                detail,
            ));
        }
        Ok(v)
    }

    /// "An installed app cannot do this", in the words a person can act on.
    ///
    /// Not `NotFound`, which reads as "that message is gone", and not a
    /// silent success, which is worse than either: the interface shows this
    /// text, and it has to name the route rather than the endpoint.
    fn unsupported(what: &'static str) -> SlackError {
        SlackError::new(
            what,
            ErrorKind::Permission,
            "not available to an installed app — this needs the browser sign-in",
        )
    }

    async fn call(&self, method: &str, form: &[(&str, &str)]) -> Result<Value> {
        // Wait out a rate limit before taking a slot, not after, so a queue of
        // waiting requests does not hold the gate shut for everyone.
        if let Some(left) = self.paused_for().await {
            tokio::time::sleep(left).await;
        }
        let _permit = self.gate.acquire().await.ok();
        // On Grid, `team_id` disambiguates which workspace in the org the
        // call is about. Appended rather than merged into the caller's form,
        // so a method that already sets its own wins.
        let team = self.team.as_str().to_string();
        let mut form = form.to_vec();
        if *self.grid.lock().await && !form.iter().any(|(k, _)| *k == "team_id") {
            form.push(("team_id", &team));
        }
        let form = &form[..];
        let mut req = self
            .http
            .post(format!("{}/{method}", self.base))
            // M0 measured that Bearer is accepted, which keeps the token out of
            // the request body and out of anything that logs bodies.
            .bearer_auth(&self.creds.token);
        // Only the session route carries the cookie. Sending an empty `d=` on
        // an OAuth request is not harmless: Slack has answered `invalid_auth`
        // to it, which reads as a revoked token and sends the user to sign in
        // again for no reason.
        if self.creds.route() == Route::Session {
            req = req.header("Cookie", format!("d={}", self.creds.cookie));
        }
        let res = req
            .form(form)
            .send()
            .await
            .map_err(|e| SlackError::new(method, ErrorKind::Transport, e.to_string()))?;

        if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);
            // Honour the number Slack gave rather than a guess of our own, and
            // apply it to everything: the limit is on the account, not on the
            // method that happened to hit it.
            *self.paused_until.lock().await =
                Some(std::time::Instant::now() + Duration::from_secs(retry_after));
            return Err(SlackError::new(
                method,
                ErrorKind::RateLimited { retry_after },
                "429",
            ));
        }

        let text = res
            .text()
            .await
            .map_err(|e| SlackError::new(method, ErrorKind::Transport, e.to_string()))?;
        let v: Value = serde_json::from_str(&text).map_err(|_| {
            // An HTML page here usually means a redirect to a login screen.
            SlackError::new(method, ErrorKind::Shape, "response was not JSON")
        })?;

        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v)
        } else {
            Err(SlackError::from_slack(
                method,
                v.get("error").and_then(Value::as_str).unwrap_or("unknown"),
            ))
        }
    }

    /// One conversation, in full.
    ///
    /// `conversations.open` answers with a thinner object than
    /// `conversations.info` does, and a group direct message needs the
    /// members to have a name at all.
    async fn conversation_info(&self, ch: &ChannelId) -> Result<Conversation> {
        let v = self
            .call("conversations.info", &[("channel", ch.as_str())])
            .await?;
        v.get("channel")
            .and_then(|c| Conversation::parse(&self.team, c))
            .ok_or_else(|| {
                SlackError::new("conversations.info", ErrorKind::Shape, "no channel object")
            })
    }

    fn parse_page(&self, method: &str, ch: &ChannelId, v: &Value) -> Result<Page> {
        let raw = v
            .get("messages")
            .and_then(Value::as_array)
            .ok_or_else(|| SlackError::new(method, ErrorKind::Shape, "no messages array"))?;
        let mut messages: Vec<Message> = raw
            .iter()
            .filter_map(|m| Message::parse(&self.team, ch, &self.self_id, m))
            .collect();
        // Slack returns newest first; every consumer here wants oldest first.
        messages.reverse();
        Ok(Page {
            messages,
            has_more: v.get("has_more").and_then(Value::as_bool).unwrap_or(false),
            cursor: v
                .get("response_metadata")
                .and_then(|m| m.get("next_cursor"))
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string),
        })
    }
}

#[async_trait]
impl SlackBackend for WebBackend {
    fn team(&self) -> &TeamId {
        &self.team
    }
    fn self_id(&self) -> &UserId {
        &self.self_id
    }
    fn capabilities(&self) -> Capabilities {
        match self.creds.route() {
            Route::Session => Capabilities::session(),
            Route::OAuth => Capabilities::oauth(!self.creds.app_token.is_empty()),
        }
    }

    /// How much longer every request is holding off, if it is.
    async fn paused_for(&self) -> Option<Duration> {
        let until = *self.paused_until.lock().await;
        until.and_then(|t| t.checked_duration_since(std::time::Instant::now()))
    }

    async fn whoami(&self) -> Result<Workspace> {
        let v = self.call("auth.test", &[]).await?;
        // FR-A6. On Enterprise Grid the session is the *org's*, and several
        // endpoints then answer for the wrong workspace unless `team_id`
        // says which one. `auth.test` reports `enterprise_id` when that is
        // the case; it is remembered here and sent from then on. Verified
        // against the shape Slack documents, **not** against a Grid org —
        // there is no Grid workspace to test with, and that is written down
        // in M3-STATUS rather than implied by a green suite.
        if v.get("enterprise_id").and_then(Value::as_str).is_some() {
            *self.grid.lock().await = true;
        }
        Ok(Workspace {
            id: TeamId::new(v["team_id"].as_str().unwrap_or_default()),
            name: v["team"].as_str().unwrap_or_default().to_string(),
            domain: self.creds.domain.clone(),
            self_id: UserId::new(v["user_id"].as_str().unwrap_or_default()),
        })
    }

    async fn boot(&self) -> Result<Boot> {
        if self.creds.route() == Route::OAuth {
            return self.boot_oauth().await;
        }
        let v = self
            .call("client.userBoot", &[("_x_reason", "initial-data")])
            .await?;

        let mut conversations = Vec::new();
        for key in ["channels", "ims", "mpims"] {
            if let Some(a) = v.get(key).and_then(Value::as_array) {
                conversations.extend(a.iter().filter_map(|c| Conversation::parse(&self.team, c)));
            }
        }

        // Muted channels live in prefs as one comma-separated string, not on
        // the channel object. A muted conversation must never raise a badge, so
        // this is not optional decoration.
        let muted = v
            .get("prefs")
            .and_then(|p| p.get("muted_channels"))
            .and_then(Value::as_str)
            .map(|s| {
                s.split(',')
                    .filter(|x| !x.is_empty())
                    .map(ChannelId::new)
                    .collect()
            })
            .unwrap_or_default();

        // `all_notifications_prefs` is a JSON *string* inside the prefs
        // object — Slack nests one document in another rather than sending
        // one. Everything about it is undocumented, so every step is an
        // `Option` and an unreadable value costs the preference, not the boot.
        let notify = v
            .get("prefs")
            .and_then(|p| p.get("all_notifications_prefs"))
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|doc| {
                let channels = doc.get("channels")?.as_object()?.clone();
                Some(
                    channels
                        .into_iter()
                        .filter_map(|(id, p)| {
                            let want = match p.get("desktop")?.as_str()? {
                                "everything" => slk_core::NotifyPref::All,
                                "mentions" => slk_core::NotifyPref::Mentions,
                                "never" => slk_core::NotifyPref::Nothing,
                                // "default" and anything Slack adds later.
                                _ => return None,
                            };
                            Some((ChannelId::new(id), want))
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();

        Ok(Boot {
            conversations,
            users: Vec::new(),
            muted,
            notify,
        })
    }

    async fn counts(&self) -> Result<Counts> {
        // `client.counts` is not a public method. The engine already asks
        // `capabilities().counts` first, so this only ever runs if something
        // stopped asking — and then it should say why rather than 404.
        if self.creds.route() == Route::OAuth {
            return Err(SlackError::new(
                "client.counts",
                ErrorKind::Permission,
                "an installed app cannot read the unread counts in one call",
            ));
        }
        let v = self
            .call(
                "client.counts",
                &[
                    ("thread_counts_by_channel", "true"),
                    ("org_wide_aware", "true"),
                ],
            )
            .await?;

        let mut conversations = Vec::new();
        for key in ["channels", "ims", "mpims"] {
            let Some(a) = v.get(key).and_then(Value::as_array) else {
                continue;
            };
            for c in a {
                let Some(id) = c.get("id").and_then(Value::as_str) else {
                    continue;
                };
                conversations.push(CountEntry {
                    id: ChannelId::new(id),
                    last_read: c.get("last_read").and_then(Value::as_str).map(Ts::new),
                    latest: c.get("latest").and_then(Value::as_str).map(Ts::new),
                    unread: c.get("mention_count").and_then(Value::as_u64).unwrap_or(0) as u32,
                    mentions: c.get("mention_count").and_then(Value::as_u64).unwrap_or(0) as u32,
                    history_invalid: c
                        .get("history_invalid")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
        }
        Ok(Counts { conversations })
    }

    async fn history(&self, ch: &ChannelId, q: HistoryQuery) -> Result<Page> {
        let limit = q.limit.to_string();
        let mut form: Vec<(&str, &str)> = vec![("channel", ch.as_str()), ("limit", &limit)];
        if let Some(l) = &q.latest {
            form.push(("latest", l.as_str()));
        }
        if let Some(o) = &q.oldest {
            form.push(("oldest", o.as_str()));
        }
        if let Some(c) = &q.cursor {
            form.push(("cursor", c));
        }
        if q.inclusive {
            form.push(("inclusive", "true"));
        }
        let v = self.call("conversations.history", &form).await?;
        self.parse_page("conversations.history", ch, &v)
    }

    async fn replies(&self, ch: &ChannelId, thread: &Ts, q: HistoryQuery) -> Result<Page> {
        let limit = q.limit.to_string();
        let form: Vec<(&str, &str)> = vec![
            ("channel", ch.as_str()),
            ("ts", thread.as_str()),
            ("limit", &limit),
        ];
        let v = self.call("conversations.replies", &form).await?;
        self.parse_page("conversations.replies", ch, &v)
    }

    async fn users(&self) -> Result<Vec<User>> {
        let v = self.call("users.list", &[("limit", "500")]).await?;
        Ok(v.get("members")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|u| User::parse(&self.team, u))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn usergroups(&self) -> Result<Vec<(String, String)>> {
        let v = self.call("usergroups.list", &[]).await?;
        Ok(v.get("usergroups")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|g| {
                        let id = g.get("id").and_then(Value::as_str)?;
                        // The handle is what a mention writes; the name is
                        // prose. `@design-team` is the handle.
                        let handle = g
                            .get("handle")
                            .and_then(Value::as_str)
                            .filter(|h| !h.is_empty())
                            .or_else(|| g.get("name").and_then(Value::as_str))?;
                        Some((id.to_string(), handle.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn custom_emoji(&self) -> Result<Vec<(String, String)>> {
        let v = self.call("emoji.list", &[]).await?;
        let Some(map) = v.get("emoji").and_then(Value::as_object) else {
            return Ok(Vec::new());
        };
        // `alias:other`, and `other` can itself be an alias. Bounded rather
        // than recursive: a workspace with a cycle in its emoji table would
        // otherwise hang the boot, and somebody has certainly made one.
        let resolve = |start: &str| -> Option<String> {
            let mut name = start.to_string();
            for _ in 0..8 {
                let val = map.get(&name)?.as_str()?;
                match val.strip_prefix("alias:") {
                    Some(next) => name = next.to_string(),
                    None => return Some(val.to_string()),
                }
            }
            None
        };
        Ok(map
            .keys()
            .filter_map(|k| Some((k.clone(), resolve(k)?)))
            // Slack's own aliases point at unicode rather than at a URL.
            .filter(|(_, url)| url.starts_with("http"))
            .collect())
    }

    async fn search_files(&self, query: &str, count: u16) -> Result<Vec<FileHit>> {
        let count = count.to_string();
        let v = self
            .call(
                "search.files",
                &[("query", query), ("count", &count), ("sort", "timestamp")],
            )
            .await?;
        let matches = v
            .get("files")
            .and_then(|m| m.get("matches"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(matches
            .iter()
            .filter_map(|f| {
                Some(FileHit {
                    id: FileId::new(f.get("id")?.as_str()?),
                    name: f
                        .get("name")
                        .and_then(Value::as_str)
                        .or_else(|| f.get("title").and_then(Value::as_str))
                        .unwrap_or("(unnamed)")
                        .to_string(),
                    mimetype: f
                        .get("mimetype")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    size: f.get("size").and_then(Value::as_u64).unwrap_or(0),
                    url_private: f
                        .get("url_private")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    // `channels` is a list; a file shared in several places
                    // gets the first, because a row can only go to one.
                    channel: f
                        .get("channels")
                        .and_then(Value::as_array)
                        .and_then(|a| a.first())
                        .and_then(Value::as_str)
                        .map(ChannelId::new),
                    channel_name: String::new(),
                    user: f.get("user").and_then(Value::as_str).map(UserId::new),
                })
            })
            .collect())
    }

    async fn public_channels(&self, limit: u16) -> Result<Vec<Conversation>> {
        let n = limit.to_string();
        let v = self
            .call(
                "conversations.list",
                &[
                    ("types", "public_channel"),
                    ("exclude_archived", "true"),
                    ("limit", &n),
                ],
            )
            .await?;
        Ok(v.get("channels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| Conversation::parse(&self.team, c))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn members(&self, ch: &ChannelId) -> Result<Vec<UserId>> {
        let v = self
            .call(
                "conversations.members",
                &[("channel", ch.as_str()), ("limit", "200")],
            )
            .await?;
        Ok(v.get("members")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(UserId::new))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn user_info(&self, id: &UserId) -> Result<User> {
        let v = self.call("users.info", &[("user", id.as_str())]).await?;
        v.get("user")
            .and_then(|u| User::parse(&self.team, u))
            .ok_or_else(|| SlackError::new("users.info", ErrorKind::Shape, "no user object"))
    }

    async fn search(&self, query: &str, count: u16) -> Result<Vec<SearchHit>> {
        let count = count.to_string();
        let v = self
            .call(
                "search.messages",
                &[("query", query), ("count", &count), ("sort", "timestamp")],
            )
            .await?;
        let matches = v
            .get("messages")
            .and_then(|m| m.get("matches"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(matches
            .iter()
            .filter_map(|m| {
                Some(SearchHit {
                    channel: ChannelId::new(m.get("channel").and_then(|c| c.get("id"))?.as_str()?),
                    channel_name: m
                        .get("channel")
                        .and_then(|c| c.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    ts: Ts::new(m.get("ts")?.as_str()?),
                    user: m.get("user").and_then(Value::as_str).map(UserId::new),
                    text: m
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                })
            })
            .collect())
    }

    async fn post(
        &self,
        ch: &ChannelId,
        thread: Option<&Ts>,
        text: &str,
        client_msg_id: &str,
        broadcast: bool,
    ) -> Result<Ts> {
        let mut form: Vec<(&str, &str)> = vec![
            ("channel", ch.as_str()),
            ("text", text),
            ("client_msg_id", client_msg_id),
        ];
        if let Some(t) = thread {
            form.push(("thread_ts", t.as_str()));
            // Slack ignores the flag without a thread_ts, but sending it there
            // anyway is the kind of thing that starts being an error later.
            if broadcast {
                form.push(("reply_broadcast", "true"));
            }
        }
        let v = self.call("chat.postMessage", &form).await?;
        Ok(Ts::new(v["ts"].as_str().unwrap_or_default()))
    }

    /// Follow or stop following a thread.
    ///
    /// `subscriptions.thread.*` is a session-route method; the official app
    /// route has no equivalent, which is why the capability is declared rather
    /// than assumed.
    async fn follow_thread(&self, ch: &ChannelId, thread: &Ts, on: bool) -> Result<()> {
        if self.creds.route() == Route::OAuth {
            return Err(Self::unsupported("subscriptions.thread"));
        }
        let method = if on {
            "subscriptions.thread.add"
        } else {
            "subscriptions.thread.remove"
        };
        match self
            .call(
                method,
                &[("channel", ch.as_str()), ("thread_ts", thread.as_str())],
            )
            .await
        {
            Ok(_) => Ok(()),
            // Following twice, or unfollowing what was never followed, is what
            // two clients racing looks like.
            Err(e) if e.detail == "already_subscribed" || e.detail == "not_subscribed" => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn mark(&self, ch: &ChannelId, ts: &Ts) -> Result<()> {
        self.call(
            "conversations.mark",
            &[("channel", ch.as_str()), ("ts", ts.as_str())],
        )
        .await?;
        Ok(())
    }

    async fn react(&self, ch: &ChannelId, ts: &Ts, name: &str, on: bool) -> Result<()> {
        let method = if on {
            "reactions.add"
        } else {
            "reactions.remove"
        };
        let form = [
            ("channel", ch.as_str()),
            ("timestamp", ts.as_str()),
            ("name", name),
        ];
        match self.call(method, &form).await {
            Ok(_) => Ok(()),
            // Reacting twice, or removing one that is already gone, is what
            // happens when two clients race. It is not worth a toast.
            Err(e) if e.detail == "already_reacted" || e.detail == "no_reaction" => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn edit(&self, ch: &ChannelId, ts: &Ts, text: &str) -> Result<()> {
        self.call(
            "chat.update",
            &[
                ("channel", ch.as_str()),
                ("ts", ts.as_str()),
                ("text", text),
            ],
        )
        .await?;
        Ok(())
    }

    async fn delete(&self, ch: &ChannelId, ts: &Ts) -> Result<()> {
        self.call(
            "chat.delete",
            &[("channel", ch.as_str()), ("ts", ts.as_str())],
        )
        .await?;
        Ok(())
    }

    async fn permalink(&self, ch: &ChannelId, ts: &Ts) -> Result<String> {
        let v = self
            .call(
                "chat.getPermalink",
                &[("channel", ch.as_str()), ("message_ts", ts.as_str())],
            )
            .await?;
        v.get("permalink")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| SlackError::new("chat.getPermalink", ErrorKind::Shape, "no permalink"))
    }

    async fn me_message(&self, ch: &ChannelId, text: &str) -> Result<()> {
        self.call(
            "chat.meMessage",
            &[("channel", ch.as_str()), ("text", text)],
        )
        .await?;
        Ok(())
    }

    async fn set_presence(&self, active: bool) -> Result<()> {
        let value = if active { "auto" } else { "away" };
        self.call("users.setPresence", &[("presence", value)])
            .await?;
        Ok(())
    }

    async fn set_status(&self, text: &str, emoji: &str, expires: i64) -> Result<()> {
        let emoji = if emoji.is_empty() || emoji.starts_with(':') {
            emoji.to_string()
        } else {
            format!(":{emoji}:")
        };
        let profile = serde_json::json!({
            "status_text": text,
            "status_emoji": emoji,
            "status_expiration": expires,
        })
        .to_string();
        self.call("users.profile.set", &[("profile", &profile)])
            .await?;
        Ok(())
    }

    async fn snooze(&self, minutes: u32) -> Result<()> {
        if minutes == 0 {
            self.call("dnd.endSnooze", &[]).await?;
        } else {
            let m = minutes.to_string();
            self.call("dnd.setSnooze", &[("num_minutes", &m)]).await?;
        }
        Ok(())
    }

    async fn channel_op(&self, op: ChannelOp) -> Result<()> {
        match op {
            ChannelOp::Join(ch) => {
                self.call("conversations.join", &[("channel", ch.as_str())])
                    .await?;
            }
            ChannelOp::Leave(ch) => {
                self.call("conversations.leave", &[("channel", ch.as_str())])
                    .await?;
            }
            ChannelOp::SetTopic(ch, t) => {
                self.call(
                    "conversations.setTopic",
                    &[("channel", ch.as_str()), ("topic", &t)],
                )
                .await?;
            }
            ChannelOp::SetPurpose(ch, t) => {
                self.call(
                    "conversations.setPurpose",
                    &[("channel", ch.as_str()), ("purpose", &t)],
                )
                .await?;
            }
            ChannelOp::Invite(ch, u) => {
                self.call(
                    "conversations.invite",
                    &[("channel", ch.as_str()), ("users", u.as_str())],
                )
                .await?;
            }
            ChannelOp::Star(ch, on) => {
                let method = if on { "stars.add" } else { "stars.remove" };
                match self.call(method, &[("channel", ch.as_str())]).await {
                    Ok(_) => {}
                    // Two clients racing, or a sidebar that was already right.
                    Err(e) if e.detail == "already_starred" || e.detail == "not_starred" => {}
                    Err(e) => return Err(e),
                }
            }
            ChannelOp::Archive(ch) => {
                match self
                    .call("conversations.archive", &[("channel", ch.as_str())])
                    .await
                {
                    Ok(_) => {}
                    // Already archived is the state that was asked for.
                    Err(e) if e.detail == "already_archived" => {}
                    Err(e) => return Err(e),
                }
            }
            ChannelOp::Rename(ch, name) => {
                self.call(
                    "conversations.rename",
                    &[("channel", ch.as_str()), ("name", &name)],
                )
                .await?;
            }
            ChannelOp::SetMuted(all) => {
                // Muting is a user preference, and `users.prefs.set` is not a
                // public method. The interface shows the refusal rather than
                // leaving a mute that looks applied until the next boot.
                if self.creds.route() == Route::OAuth {
                    return Err(Self::unsupported("users.prefs.set"));
                }
                let value = all.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(",");
                self.call(
                    "users.prefs.set",
                    &[("name", "muted_channels"), ("value", &value)],
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn create_channel(&self, name: &str, private: bool) -> Result<Conversation> {
        let v = self
            .call(
                "conversations.create",
                &[
                    ("name", name),
                    ("is_private", if private { "true" } else { "false" }),
                ],
            )
            .await?;
        v.get("channel")
            .and_then(|c| Conversation::parse(&self.team, c))
            .ok_or_else(|| {
                SlackError::new(
                    "conversations.create",
                    ErrorKind::Shape,
                    "no channel object",
                )
            })
    }

    async fn open_group(&self, users: &[UserId]) -> Result<Conversation> {
        let list = users
            .iter()
            .map(|u| u.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let v = self
            .call(
                "conversations.open",
                // `return_im` is what makes the answer a channel object
                // rather than the bare `{"id": ...}` Slack sends otherwise —
                // and a bare id has no name, no members and no kind, so the
                // sidebar would show a row with nothing in it.
                &[("users", &list), ("return_im", "true")],
            )
            .await?;
        let parsed = v
            .get("channel")
            .and_then(|c| Conversation::parse(&self.team, c));
        match parsed {
            // A group whose object came back without its members is a group
            // the sidebar cannot name. Ask properly rather than show `@`.
            Some(c) if !c.name.is_empty() || users.len() == 1 => Ok(c),
            Some(c) => self.conversation_info(&c.id).await.or(Ok(c)),
            None => Err(SlackError::new(
                "conversations.open",
                ErrorKind::Shape,
                "no channel object",
            )),
        }
    }

    async fn slash(&self, ch: &ChannelId, command: &str, text: &str) -> Result<()> {
        if self.creds.route() == Route::OAuth {
            return Err(Self::unsupported("chat.command"));
        }
        // `chat.command` is the web client's own path, and the only one that
        // reaches an app's slash command as the user.
        self.call(
            "chat.command",
            &[
                ("channel", ch.as_str()),
                ("command", command),
                ("text", text),
            ],
        )
        .await?;
        Ok(())
    }

    async fn save(&self, ch: &ChannelId, ts: &Ts, on: bool) -> Result<()> {
        // `saved.*` is what the web client uses, and its argument shape is not
        // written down anywhere — measured against a real workspace, both the
        // obvious guesses come back `invalid_arguments`. `stars.*` still
        // answers and still works, so it is what actually runs until someone
        // captures what the web client really sends.
        //
        // The fallback fires on the two answers that mean "this endpoint is
        // not what you think it is", and on nothing else: a genuine auth
        // failure has to surface rather than be retried against another method.
        // An installed app has `stars:write` and no `saved.*` at all, so it
        // goes straight to the endpoint the fallback would have reached.
        if self.creds.route() == Route::OAuth {
            let legacy = if on { "stars.add" } else { "stars.remove" };
            return match self
                .call(
                    legacy,
                    &[("channel", ch.as_str()), ("timestamp", ts.as_str())],
                )
                .await
            {
                Ok(_) => Ok(()),
                Err(e) if e.detail == "already_starred" || e.detail == "not_starred" => Ok(()),
                Err(e) => Err(e),
            };
        }
        let method = if on { "saved.add" } else { "saved.remove" };
        let form = [("channel", ch.as_str()), ("ts", ts.as_str())];
        match self.call(method, &form).await {
            Ok(_) => Ok(()),
            Err(e) if e.detail == "unknown_method" || e.detail == "invalid_arguments" => {
                let legacy = if on { "stars.add" } else { "stars.remove" };
                match self
                    .call(
                        legacy,
                        &[("channel", ch.as_str()), ("timestamp", ts.as_str())],
                    )
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(e) if e.detail == "already_starred" || e.detail == "not_starred" => Ok(()),
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn pin(&self, ch: &ChannelId, ts: &Ts, on: bool) -> Result<()> {
        let method = if on { "pins.add" } else { "pins.remove" };
        match self
            .call(
                method,
                &[("channel", ch.as_str()), ("timestamp", ts.as_str())],
            )
            .await
        {
            Ok(_) => Ok(()),
            // Pinning twice is what two clients racing looks like.
            Err(e) if e.detail == "already_pinned" || e.detail == "no_pin" => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn pins(&self, ch: &ChannelId) -> Result<Vec<Ts>> {
        let v = self.call("pins.list", &[("channel", ch.as_str())]).await?;
        // `pins.list` returns items, not messages: a pinned file has no `ts`
        // of its own and is skipped rather than guessed at.
        Ok(v.get("items")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|i| i.get("message")?.get("ts")?.as_str())
                    .map(Ts::new)
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn drafts(&self) -> Result<Vec<RemoteDraft>> {
        if self.creds.route() == Route::OAuth {
            return Ok(Vec::new());
        }
        let v = self.call("drafts.list", &[]).await?;
        Ok(parse_drafts(&v))
    }

    async fn save_draft(
        &self,
        id: Option<&str>,
        ch: &ChannelId,
        thread: Option<&Ts>,
        text: &str,
        last: Option<&Ts>,
    ) -> Result<(String, Ts)> {
        if self.creds.route() == Route::OAuth {
            return Err(Self::unsupported("drafts.create"));
        }
        let blocks = draft_blocks(text).to_string();
        let mut dest = serde_json::json!({ "channel_id": ch.as_str() });
        if let Some(t) = thread {
            dest["thread_ts"] = serde_json::json!(t.as_str());
        }
        let destinations = serde_json::json!([dest]).to_string();
        let v = match id {
            Some(id) => {
                let last = last.map(Ts::as_str).unwrap_or("");
                self.call(
                    "drafts.update",
                    &[
                        ("draft_id", id),
                        ("client_last_updated_ts", last),
                        ("blocks", &blocks),
                        ("destinations", &destinations),
                        ("file_ids", "[]"),
                    ],
                )
                .await?
            }
            None => {
                let msg_id = client_msg_id();
                self.call(
                    "drafts.create",
                    &[
                        ("client_msg_id", &msg_id),
                        ("blocks", &blocks),
                        ("destinations", &destinations),
                        ("file_ids", "[]"),
                        ("is_from_composer", "false"),
                    ],
                )
                .await?
            }
        };
        let d = v.get("draft");
        let new_id = d
            .and_then(|d| d.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| id.map(str::to_string));
        let ts = d
            .and_then(|d| d.get("last_updated_ts"))
            .and_then(Value::as_str)
            .map(Ts::new);
        match (new_id, ts) {
            (Some(i), Some(t)) => Ok((i, t)),
            _ => Err(SlackError::new(
                "drafts",
                ErrorKind::Shape,
                "no draft object",
            )),
        }
    }

    async fn delete_draft(&self, id: &str, last: Option<&Ts>) -> Result<()> {
        if self.creds.route() == Route::OAuth {
            return Err(Self::unsupported("drafts.delete"));
        }
        let last = last.map(Ts::as_str).unwrap_or("");
        match self
            .call(
                "drafts.delete",
                &[("draft_id", id), ("client_last_updated_ts", last)],
            )
            .await
        {
            Ok(_) => Ok(()),
            // Gone already — finished on another device, which is the state
            // that was asked for.
            Err(e) if e.kind == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn sections(&self) -> Result<Vec<SidebarSection>> {
        // Not a public method, and asking would cost a request to be told
        // so. The official route's sidebar is the built-in one.
        if self.creds.route() == Route::OAuth {
            return Ok(Vec::new());
        }
        let v = self.call("users.channelSections.list", &[]).await?;
        Ok(parse_sections(&v))
    }

    async fn bookmarks(&self, ch: &ChannelId) -> Result<Vec<Bookmark>> {
        let v = self
            .call("bookmarks.list", &[("channel_id", ch.as_str())])
            .await?;
        Ok(parse_bookmarks(&v))
    }

    async fn download(&self, url: &str, to: &std::path::Path) -> Result<u64> {
        let _permit = self.gate.acquire().await.ok();
        let res = self
            .http
            .get(url)
            .header("Cookie", format!("d={}", self.creds.cookie))
            .bearer_auth(&self.creds.token)
            .send()
            .await
            .map_err(|e| SlackError::new("download", ErrorKind::Transport, e.to_string()))?;

        if !res.status().is_success() {
            return Err(SlackError::new(
                "download",
                if res.status().as_u16() == 403 {
                    ErrorKind::Permission
                } else {
                    ErrorKind::Transport
                },
                res.status().to_string(),
            ));
        }
        // A sign-in page arrives with a 200, so the content type is the only
        // way to tell a file from a redirect to the login screen.
        let html = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("text/html"))
            .unwrap_or(false);
        let bytes = res
            .bytes()
            .await
            .map_err(|e| SlackError::new("download", ErrorKind::Transport, e.to_string()))?;
        if html {
            return Err(SlackError::new(
                "download",
                ErrorKind::Auth,
                "got a sign-in page instead of the file",
            ));
        }
        if let Some(dir) = to.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(to, &bytes)
            .map_err(|e| SlackError::new("download", ErrorKind::Transport, e.to_string()))?;
        Ok(bytes.len() as u64)
    }

    async fn upload(
        &self,
        ch: &ChannelId,
        thread: Option<&Ts>,
        path: &std::path::Path,
        comment: Option<&str>,
    ) -> Result<()> {
        let bytes = std::fs::read(path)
            .map_err(|e| SlackError::new("upload", ErrorKind::Transport, e.to_string()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        let len = bytes.len().to_string();

        // Three steps since files.upload was retired in March 2025: ask for a
        // URL, PUT the bytes at it, then tell Slack where to attach them.
        let v = self
            .call(
                "files.getUploadURLExternal",
                &[("filename", &name), ("length", &len)],
            )
            .await?;
        let (Some(url), Some(file_id)) = (
            v.get("upload_url").and_then(Value::as_str),
            v.get("file_id").and_then(Value::as_str),
        ) else {
            return Err(SlackError::new(
                "files.getUploadURLExternal",
                ErrorKind::Shape,
                "no upload_url",
            ));
        };

        self.http
            .post(url)
            .body(bytes)
            .send()
            .await
            .map_err(|e| SlackError::new("upload", ErrorKind::Transport, e.to_string()))?;

        let files = serde_json::json!([{ "id": file_id, "title": name }]).to_string();
        let mut form: Vec<(&str, &str)> = vec![("files", &files), ("channel_id", ch.as_str())];
        if let Some(t) = thread {
            form.push(("thread_ts", t.as_str()));
        }
        if let Some(c) = comment {
            form.push(("initial_comment", c));
        }
        self.call("files.completeUploadExternal", &form).await?;
        Ok(())
    }

    async fn typing(&self, ch: &ChannelId) -> Result<()> {
        // Socket Mode is inbound only; there is no frame to send. Silently,
        // because `capabilities().typing` already said so and an indicator
        // must never be the reason anything fails.
        if self.creds.route() == Route::OAuth {
            return Ok(());
        }
        // Down the socket the client already has. Dropped silently when there
        // is no socket, and never retried: by the time a retry landed the
        // person has either sent the message or stopped typing.
        if let Some(tx) = self.outbound.lock().await.clone() {
            let id = self
                .frame_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = tx.try_send(format!(
                r#"{{"id":{id},"type":"typing","channel":"{}"}}"#,
                ch.as_str()
            ));
        }
        Ok(())
    }

    async fn connect(&self, presence: &[UserId]) -> Result<Option<EventStream>> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        let url = if self.creds.route() == Route::OAuth {
            // Socket Mode, the only realtime an installed app has. It needs an
            // app-level token, which OAuth does not hand out — the person
            // pastes it. Without one there is no stream, and `capabilities()`
            // already said so, so the engine polls rather than waiting for
            // events that will never come.
            if self.creds.app_token.is_empty() {
                return Ok(None);
            }
            let v = self.app_call("apps.connections.open").await?;
            match v.get("url").and_then(Value::as_str) {
                Some(u) => u.to_string(),
                None => {
                    return Err(SlackError::new(
                        "apps.connections.open",
                        ErrorKind::Shape,
                        "no url",
                    ))
                }
            }
        } else {
            // Prefer the URL Slack pushed. M0 measured that an old handshake
            // URL also still works, but this is the path Slack intends.
            match self.reconnect_url.lock().await.clone() {
                Some(u) => u,
                None => format!(
                    "wss://wss-primary.slack.com/?token={}&gateway_server={}-1&slack_client=desktop&batch_presence_aware=1",
                    self.creds.token,
                    self.team.as_str()
                ),
            }
        };

        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| SlackError::new("ws", ErrorKind::Transport, e.to_string()))?;
        let hdr = |v: String| {
            v.parse()
                .map_err(|_| SlackError::new("ws", ErrorKind::Transport, "bad header"))
        };
        req.headers_mut()
            .insert("Cookie", hdr(format!("d={}", self.creds.cookie))?);
        req.headers_mut().insert(
            "Origin",
            hdr(format!("https://{}.slack.com", self.creds.domain))?,
        );

        let (ws, _) = tokio_tungstenite::connect_async(req)
            .await
            .map_err(|e| SlackError::new("ws", ErrorKind::Transport, e.to_string()))?;

        let socket_mode = self.creds.route() == Route::OAuth;
        let (tx, rx) = mpsc::channel(256);
        // Frames this client wants to send: typing, and nothing else so far.
        // Bounded and `try_send`-only, so a stalled socket drops indicators
        // rather than backing up into whatever is calling.
        let (otx, mut orx) = mpsc::channel::<String>(8);
        *self.outbound.lock().await = Some(otx);
        let reconnect_url = self.reconnect_url.clone();
        let mut watch: Vec<String> = presence.iter().map(|u| u.as_str().to_string()).collect();
        watch.push(self.self_id.as_str().to_string());
        watch.sort();
        watch.dedup();

        tokio::spawn(async move {
            let (mut sink, mut stream) = ws.split();
            // emacs-slack's numbers, and M0 measured a 21 ms round trip: a
            // socket that stops answering is the normal failure here, not an
            // exceptional one.
            let mut ping = tokio::time::interval(Duration::from_secs(10));
            let mut id = 1i64;
            let mut subscribed = false;

            loop {
                tokio::select! {
                    _ = ping.tick() => {
                        id += 1;
                        if sink
                            .send(WsMessage::Text(format!(r#"{{"id":{id},"type":"ping"}}"#).into()))
                            .await
                            .is_err()
                        {
                            let _ = tx.send(RtEvent::Disconnected("ping failed".into())).await;
                            return;
                        }
                    }
                    out = orx.recv() => {
                        let Some(out) = out else { continue };
                        if sink.send(WsMessage::Text(out.into())).await.is_err() {
                            let _ = tx.send(RtEvent::Disconnected("send failed".into())).await;
                            return;
                        }
                    }
                    frame = stream.next() => {
                        let Some(frame) = frame else {
                            let _ = tx.send(RtEvent::Disconnected("stream ended".into())).await;
                            return;
                        };
                        match frame {
                            Ok(WsMessage::Text(t)) => {
                                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                                // Socket Mode wraps every event in an
                                // envelope and expects it acknowledged; an
                                // unacknowledged envelope is redelivered
                                // three times and then the connection is
                                // dropped, which reads as a flaky network.
                                let v = if socket_mode {
                                    if let Some(id) = v.get("envelope_id").and_then(Value::as_str) {
                                        let ack = format!(r#"{{"envelope_id":"{id}"}}"#);
                                        let _ = sink.send(WsMessage::Text(ack.into())).await;
                                    }
                                    match v.get("payload").and_then(|p| p.get("event")) {
                                        Some(e) => e.clone(),
                                        // `hello`, `disconnect` and the
                                        // acknowledgements of our own acks
                                        // carry no event.
                                        None => v.clone(),
                                    }
                                } else {
                                    v
                                };
                                let Some(ev) = events::parse(&v) else { continue };
                                match &ev {
                                    // Presence is one of the two features the
                                    // session route exists for, and Slack
                                    // reports none until asked. Socket Mode
                                    // has no such subscription — it is an
                                    // events stream, not a client socket.
                                    RtEvent::Hello if !subscribed && !socket_mode => {
                                        subscribed = true;
                                        let ids = watch
                                            .iter()
                                            .map(|i| format!("\"{i}\""))
                                            .collect::<Vec<_>>()
                                            .join(",");
                                        let sub =
                                            format!(r#"{{"type":"presence_sub","ids":[{ids}]}}"#);
                                        let _ = sink.send(WsMessage::Text(sub.into())).await;
                                    }
                                    RtEvent::ReconnectUrl(u) => {
                                        *reconnect_url.lock().await = Some(u.clone());
                                    }
                                    _ => {}
                                }
                                if tx.send(ev).await.is_err() {
                                    return; // the engine went away
                                }
                            }
                            Ok(WsMessage::Ping(p)) => {
                                let _ = sink.send(WsMessage::Pong(p)).await;
                            }
                            Ok(WsMessage::Close(c)) => {
                                let _ = tx
                                    .send(RtEvent::Disconnected(format!("{c:?}")))
                                    .await;
                                return;
                            }
                            Err(e) => {
                                let _ = tx.send(RtEvent::Disconnected(e.to_string())).await;
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok(Some(rx))
    }
}

/// The link bookmarks in a `bookmarks.list` answer, in the order Slack sent
/// them.
///
/// Slack bookmarks four kinds and this client can open one. A folder has no
/// link at all; a canvas and a file link into the web client, which is not
/// what a bar on a native window should quietly do. So the bar shows what it
/// can open and nothing else, rather than a row that does nothing when
/// clicked.
///
/// A free function rather than a method so the filter can be tested without a
/// socket — the parse is the part that goes wrong when Slack changes, and the
/// request is the part that cannot be tested here at all.
fn parse_bookmarks(v: &Value) -> Vec<Bookmark> {
    v.get("bookmarks")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("link"))
                .filter_map(|b| {
                    let link = b.get("link")?.as_str()?;
                    if link.is_empty() {
                        return None;
                    }
                    Some(Bookmark {
                        id: b.get("id")?.as_str()?.to_string(),
                        title: b
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        link: link.to_string(),
                        emoji: b
                            .get("emoji")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The sections in a `users.channelSections.list` answer, in the order the
/// person sees them.
///
/// Slack does not send them in order. Each section names the one after it in
/// `next_channel_section_id`, and the first is the one nobody names. The
/// chain is followed from there; if it is broken — no head, a loop, a name
/// that points nowhere — what the chain did not reach is appended in array
/// order, so a malformed answer costs the order and never a section.
///
/// Undocumented, and never captured from `slk-dev`: every accessor is an
/// `Option`, a section with no id is dropped, and a type this client has not
/// seen is `Other` rather than a failure.
fn parse_sections(v: &Value) -> Vec<SidebarSection> {
    let Some(raw) = v.get("channel_sections").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut all: Vec<(SidebarSection, Option<String>)> = raw
        .iter()
        .filter_map(|x| {
            let id = x.get("channel_section_id")?.as_str()?.to_string();
            let kind = match x.get("type").and_then(Value::as_str) {
                Some("standard") => SectionKind::Custom,
                Some("stars") => SectionKind::Starred,
                Some("channels") => SectionKind::Channels,
                Some("direct_messages") => SectionKind::Dms,
                _ => SectionKind::Other,
            };
            let channels = x
                .get("channel_ids_page")
                .and_then(|p| p.get("channel_ids"))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(ChannelId::new))
                        .collect()
                })
                .unwrap_or_default();
            let next = x
                .get("next_channel_section_id")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .map(str::to_string);
            Some((
                SidebarSection {
                    id,
                    name: x
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    emoji: x
                        .get("emoji")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim_matches(':')
                        .to_string(),
                    kind,
                    channels,
                },
                next,
            ))
        })
        .collect();

    let named: std::collections::HashSet<String> =
        all.iter().filter_map(|(_, n)| n.clone()).collect();
    let mut out: Vec<SidebarSection> = Vec::with_capacity(all.len());
    let mut at = all.iter().position(|(s, _)| !named.contains(&s.id));
    // Bounded by the count: a loop in the chain ends the walk rather than
    // the client.
    while let Some(i) = at {
        if out.len() >= all.len() || out.iter().any(|s| s.id == all[i].0.id) {
            break;
        }
        out.push(all[i].0.clone());
        at = all[i]
            .1
            .as_ref()
            .and_then(|n| all.iter().position(|(s, _)| &s.id == n));
    }
    for (s, _) in all.drain(..) {
        if !out.iter().any(|o| o.id == s.id) {
            out.push(s);
        }
    }
    out
}

/// What a composer holds, as the one rich_text block Slack keeps a draft in.
///
/// One text element, verbatim. `@alice` stays the words `@alice` rather than
/// becoming a mention: resolving it is what *sending* does, through the same
/// encoder every message goes through, and a draft is somewhere to keep words,
/// not to send them. Slack's own client shows it as the words typed.
fn draft_blocks(text: &str) -> Value {
    serde_json::json!([{
        "type": "rich_text",
        "elements": [{
            "type": "rich_text_section",
            "elements": [{ "type": "text", "text": text }]
        }]
    }])
}

/// A fresh `client_msg_id`, in the UUID shape Slack's own client sends.
/// Unique rather than random — nanoseconds and a counter — which is all an
/// id for a draft has to be.
fn client_msg_id() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = u128::from(N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let h = format!("{:032x}", nanos ^ (n << 64));
    format!(
        "{}-{}-4{}-a{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}

/// The drafts in a `drafts.list` answer that belong in a composer.
///
/// Not the deleted, not the sent, not the scheduled — Slack keeps a
/// scheduled message in the same list, and it is not a composer's contents.
/// Only a draft with exactly one destination: one addressed to several
/// conversations has no single composer to go back into. And only one with
/// words in it — a draft that is only an attached file has nothing a
/// composer can hold.
///
/// Undocumented and never captured from `slk-dev`: every accessor is an
/// `Option`, and a draft this cannot read is skipped, never fatal.
fn parse_drafts(v: &Value) -> Vec<RemoteDraft> {
    let Some(all) = v.get("drafts").and_then(Value::as_array) else {
        return Vec::new();
    };
    all.iter()
        .filter_map(|d| {
            let flag = |k: &str| d.get(k).and_then(Value::as_bool).unwrap_or(false);
            if flag("is_deleted") || flag("is_sent") {
                return None;
            }
            if d.get("date_scheduled").and_then(Value::as_i64).unwrap_or(0) > 0 {
                return None;
            }
            let [dest] = d.get("destinations")?.as_array()?.as_slice() else {
                return None;
            };
            let channel = ChannelId::new(dest.get("channel_id")?.as_str()?);
            let thread = dest
                .get("thread_ts")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(Ts::new);
            let block = d
                .get("blocks")?
                .as_array()?
                .iter()
                .find(|b| b.get("type").and_then(Value::as_str) == Some("rich_text"))?;
            let doc = slk_core::richtext::parse_block(block);
            if doc.is_empty() {
                return None;
            }
            Some(RemoteDraft {
                id: d.get("id")?.as_str()?.to_string(),
                channel,
                thread,
                doc,
                updated: Ts::new(d.get("last_updated_ts")?.as_str()?),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_kinds_that_can_be_opened() {
        let v: Value = serde_json::from_str(
            r#"{"ok":true,"bookmarks":[
              {"id":"Bk1","type":"link","title":"Runbook","link":"https://x.invalid/r","emoji":":books:"},
              {"id":"Bk2","type":"folder","title":"Docs"},
              {"id":"Bk3","type":"canvas","title":"Plan","link":"https://x.invalid/c"},
              {"id":"Bk4","type":"link","title":"Rota","link":"https://x.invalid/o"}
            ]}"#,
        )
        .unwrap();
        let got = parse_bookmarks(&v);
        // The count, not "the runbook is in there": a filter that lets the
        // canvas through still passes a test that only looks for what it
        // wanted. M3 lost a milestone to exactly that shape of assertion.
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "Bk1");
        assert_eq!(got[0].emoji, ":books:");
        assert_eq!(got[1].id, "Bk4");
        assert_eq!(got[1].emoji, "");
    }

    #[test]
    fn a_link_kind_with_no_link_is_not_a_row() {
        let v: Value = serde_json::from_str(
            r#"{"ok":true,"bookmarks":[
              {"id":"Bk1","type":"link","title":"Broken","link":""},
              {"id":"Bk2","type":"link","title":"No link at all"}
            ]}"#,
        )
        .unwrap();
        assert!(parse_bookmarks(&v).is_empty());
    }

    #[test]
    fn a_shape_we_have_never_seen_costs_the_bar_and_nothing_else() {
        for text in [r#"{"ok":true}"#, r#"{"ok":true,"bookmarks":"soon"}"#] {
            let v: Value = serde_json::from_str(text).unwrap();
            assert!(parse_bookmarks(&v).is_empty());
        }
    }

    #[test]
    fn sections_are_in_the_order_the_chain_says_not_the_array() {
        // Slack sends them in whatever order; the chain is the truth.
        let v: Value = serde_json::from_str(
            r#"{"ok":true,"channel_sections":[
              {"channel_section_id":"L3","type":"direct_messages","name":"","next_channel_section_id":null},
              {"channel_section_id":"L1","type":"stars","name":"","next_channel_section_id":"L2"},
              {"channel_section_id":"L2","type":"standard","name":"Projects","emoji":":rocket:",
               "next_channel_section_id":"L4",
               "channel_ids_page":{"channel_ids":["C1","C2"],"count":2,"cursor":null}},
              {"channel_section_id":"L4","type":"channels","name":"","next_channel_section_id":"L3"}
            ]}"#,
        )
        .unwrap();
        let got = parse_sections(&v);
        let ids: Vec<&str> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["L1", "L2", "L4", "L3"]);
        assert_eq!(got[1].kind, SectionKind::Custom);
        assert_eq!(got[1].name, "Projects");
        assert_eq!(got[1].emoji, "rocket", "colons trimmed once, here");
        assert_eq!(got[1].channels.len(), 2);
    }

    #[test]
    fn a_broken_chain_costs_the_order_and_never_a_section() {
        // A loop, and a section nothing reaches.
        let v: Value = serde_json::from_str(
            r#"{"ok":true,"channel_sections":[
              {"channel_section_id":"A","type":"standard","name":"a","next_channel_section_id":"B"},
              {"channel_section_id":"B","type":"standard","name":"b","next_channel_section_id":"A"},
              {"channel_section_id":"C","type":"salesforce_records","name":"c"},
              {"type":"standard","name":"no id at all"}
            ]}"#,
        )
        .unwrap();
        let got = parse_sections(&v);
        assert_eq!(got.len(), 3, "every section with an id, once each");
        assert_eq!(got.iter().filter(|s| s.id == "A").count(), 1);
        assert_eq!(
            got.iter().find(|s| s.id == "C").map(|s| s.kind),
            Some(SectionKind::Other),
            "a type never seen is carried, not a failure"
        );
    }

    #[test]
    fn no_sections_is_the_built_in_sidebar() {
        for text in [r#"{"ok":true}"#, r#"{"ok":true,"channel_sections":{}}"#] {
            let v: Value = serde_json::from_str(text).unwrap();
            assert!(parse_sections(&v).is_empty());
        }
    }

    #[test]
    fn only_drafts_with_one_home_and_words_in_them() {
        use serde_json::json;
        let words = |t: &str| {
            json!([{"type":"rich_text","elements":[
                {"type":"rich_text_section","elements":[{"type":"text","text":t}]}]}])
        };
        let draft = |id: &str, extra: Value| {
            let mut o = json!({
                "id": id,
                "last_updated_ts": "1725700000.000100",
                "destinations": [{"channel_id": "C1"}],
                "blocks": words("words"),
            });
            for (k, v) in extra.as_object().unwrap() {
                o[k.as_str()] = v.clone();
            }
            o
        };
        let v = json!({"ok": true, "drafts": [
            draft("Dr1", json!({"blocks": words("half a thought")})),
            draft("Dr2", json!({"destinations": [{"channel_id": "C1", "thread_ts": "1725600000.000100"}]})),
            draft("Dr3", json!({"is_sent": true})),
            draft("Dr4", json!({"is_deleted": true})),
            draft("Dr5", json!({"date_scheduled": 1999999999})),
            draft("Dr6", json!({"destinations": [{"channel_id": "C1"}, {"channel_id": "C2"}]})),
            draft("Dr7", json!({"destinations": []})),
            draft("Dr8", json!({"blocks": [], "file_ids": ["F1"]})),
        ]});
        let got = parse_drafts(&v);
        let ids: Vec<&str> = got.iter().map(|d| d.id.as_str()).collect();
        // The ids, not "Dr1 is in there": a filter letting a sent draft
        // through still passes a test that only looks for the one it wanted.
        assert_eq!(ids, ["Dr1", "Dr2"]);
        assert_eq!(got[0].doc.plain(), "half a thought");
        assert_eq!(got[0].thread, None);
        assert_eq!(
            got[1].thread.as_ref().map(|t| t.as_str()),
            Some("1725600000.000100")
        );
    }

    #[test]
    fn a_draft_is_kept_as_the_words_typed() {
        let text = "for @alice: <soon> & *now*";
        let b = draft_blocks(text);
        assert_eq!(slk_core::richtext::parse_block(&b[0]).plain(), text);
    }

    #[test]
    fn client_msg_ids_are_uuid_shaped_and_distinct() {
        let (a, b) = (client_msg_id(), client_msg_id());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
        assert_eq!(&a[14..15], "4", "the version nibble");
    }
}
