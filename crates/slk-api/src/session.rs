//! The browser-session backend: an `xoxc` token and the `d` cookie.
//!
//! This is the route the client is built on, because it is the only one that
//! reaches the endpoints the product's parity depends on — `client.counts` for
//! every unread badge in a single call, `drafts.*`, `saved.list`, and a
//! websocket that carries typing and presence. M0 measured all of it working.
//!
//! It is also undocumented, so everything here is written on the assumption
//! that Slack will change it: one module, defensive parsing, and shapes pinned
//! by fixtures rather than trusted.

use crate::backend::*;
use crate::error::{ErrorKind, Result, SlackError};
use crate::events::{self, EventStream, RtEvent};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use slk_core::{ChannelId, Conversation, Message, TeamId, Ts, User, UserId, Workspace};
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

#[derive(Clone)]
pub struct Credentials {
    /// The `<team>.slack.com` subdomain.
    pub domain: String,
    pub token: String,
    /// The value of the `d` cookie, without the `d=`.
    pub cookie: String,
}

pub struct SessionBackend {
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
}

impl SessionBackend {
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
        let mut me = SessionBackend {
            http,
            creds,
            base,
            team: TeamId::new(""),
            self_id: UserId::new(""),
            reconnect_url: Arc::new(Mutex::new(None)),
            gate: Arc::new(tokio::sync::Semaphore::new(4)),
            paused_until: Arc::new(Mutex::new(None)),
        };
        let ws = me.whoami().await?;
        me.team = ws.id.clone();
        me.self_id = ws.self_id.clone();
        Ok(me)
    }

    async fn call(&self, method: &str, form: &[(&str, &str)]) -> Result<Value> {
        // Wait out a rate limit before taking a slot, not after, so a queue of
        // waiting requests does not hold the gate shut for everyone.
        if let Some(left) = self.paused_for().await {
            tokio::time::sleep(left).await;
        }
        let _permit = self.gate.acquire().await.ok();
        let res = self
            .http
            .post(format!("{}/{method}", self.base))
            .header("Cookie", format!("d={}", self.creds.cookie))
            // M0 measured that Bearer is accepted, which keeps the token out of
            // the request body and out of anything that logs bodies.
            .bearer_auth(&self.creds.token)
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
impl SlackBackend for SessionBackend {
    fn team(&self) -> &TeamId {
        &self.team
    }
    fn self_id(&self) -> &UserId {
        &self.self_id
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::session()
    }

    /// How much longer every request is holding off, if it is.
    async fn paused_for(&self) -> Option<Duration> {
        let until = *self.paused_until.lock().await;
        until.and_then(|t| t.checked_duration_since(std::time::Instant::now()))
    }

    async fn whoami(&self) -> Result<Workspace> {
        let v = self.call("auth.test", &[]).await?;
        Ok(Workspace {
            id: TeamId::new(v["team_id"].as_str().unwrap_or_default()),
            name: v["team"].as_str().unwrap_or_default().to_string(),
            domain: self.creds.domain.clone(),
            self_id: UserId::new(v["user_id"].as_str().unwrap_or_default()),
        })
    }

    async fn boot(&self) -> Result<Boot> {
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

        Ok(Boot {
            conversations,
            users: Vec::new(),
            muted,
        })
    }

    async fn counts(&self) -> Result<Counts> {
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

    async fn set_status(&self, text: &str, emoji: &str) -> Result<()> {
        let emoji = if emoji.is_empty() || emoji.starts_with(':') {
            emoji.to_string()
        } else {
            format!(":{emoji}:")
        };
        let profile = serde_json::json!({
            "status_text": text,
            "status_emoji": emoji,
            "status_expiration": 0,
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
            ChannelOp::SetMuted(all) => {
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

    async fn slash(&self, ch: &ChannelId, command: &str, text: &str) -> Result<()> {
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

    async fn connect(&self, presence: &[UserId]) -> Result<Option<EventStream>> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        // Prefer the URL Slack pushed. M0 measured that an old handshake URL
        // also still works, but this is the path Slack intends.
        let url = match self.reconnect_url.lock().await.clone() {
            Some(u) => u,
            None => format!(
                "wss://wss-primary.slack.com/?token={}&gateway_server={}-1&slack_client=desktop&batch_presence_aware=1",
                self.creds.token,
                self.team.as_str()
            ),
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

        let (tx, rx) = mpsc::channel(256);
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
                    frame = stream.next() => {
                        let Some(frame) = frame else {
                            let _ = tx.send(RtEvent::Disconnected("stream ended".into())).await;
                            return;
                        };
                        match frame {
                            Ok(WsMessage::Text(t)) => {
                                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                                let Some(ev) = events::parse(&v) else { continue };
                                match &ev {
                                    RtEvent::Hello if !subscribed => {
                                        subscribed = true;
                                        // Presence is one of the two features
                                        // this route exists for, and Slack
                                        // reports none until asked.
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
