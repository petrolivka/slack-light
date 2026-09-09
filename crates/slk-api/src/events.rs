//! The realtime event stream, normalised.
//!
//! Slack's socket carries dozens of event types, several of which appear in no
//! documentation — M0 saw `badge_counts_updated`, `update_global_thread_state`,
//! `activity_deleted` and `user_interaction_changed`, none of them written down
//! anywhere. So the rule here is the same as for the parsers: recognise what we
//! model, keep the raw JSON for everything else, and never let an unknown shape
//! be fatal.

use serde_json::Value;
use slk_core::{ChannelId, Ts, UserId};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum RtEvent {
    /// The stream is live. Nothing before this counts.
    Hello,
    /// A new message. `raw` is kept so the store can hold it verbatim.
    Message {
        channel: ChannelId,
        ts: Ts,
        raw: Value,
    },
    MessageChanged {
        channel: ChannelId,
        ts: Ts,
        raw: Value,
    },
    MessageDeleted {
        channel: ChannelId,
        ts: Ts,
    },
    /// A reply landed in a thread; the parent's counters moved.
    ThreadReply {
        channel: ChannelId,
        thread_ts: Ts,
        raw: Value,
    },
    ReactionChanged {
        channel: ChannelId,
        ts: Ts,
        name: String,
        user: UserId,
        added: bool,
    },
    /// A read cursor moved, possibly on the user's phone. This is what keeps
    /// badges honest across devices.
    Marked {
        channel: ChannelId,
        ts: Ts,
    },
    ThreadMarked {
        channel: ChannelId,
        thread_ts: Ts,
        ts: Ts,
    },
    Typing {
        channel: ChannelId,
        user: UserId,
    },
    PresenceChanged {
        users: Vec<UserId>,
        presence: slk_core::Presence,
    },
    /// Slack handing us the URL to reconnect with. M0 measured that it sends
    /// these unprompted every few minutes, and that reusing an old URL also
    /// works — but this is the intended path.
    ReconnectUrl(String),
    /// Slack's own decision that this is worth interrupting for.
    ///
    /// FR-U4: when the backend sends these, they beat the client's own rule —
    /// they already account for keywords, per-channel settings, DND and
    /// whatever else Slack has added since. The `ts` is what lets the message
    /// that arrives alongside be skipped rather than notified twice.
    DesktopNotification {
        channel: ChannelId,
        ts: Ts,
        title: String,
        text: String,
        /// Slack's own word for it, so "mentioned you" stays true.
        is_mention: bool,
    },
    /// Recognised as a type, not modelled. Logged at debug and dropped.
    Unhandled(String),
    /// The socket closed. The engine reconnects; it does not treat this as an
    /// error, because it is the normal state of a long-lived websocket.
    Disconnected(String),
}

pub type EventStream = mpsc::Receiver<RtEvent>;

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str()
}

/// Turn one raw frame into an `RtEvent`.
///
/// Returns `None` for a frame with no `type` at all, which is either a reply to
/// something we sent or a shape we have no name for.
pub fn parse(v: &Value) -> Option<RtEvent> {
    let ty = s(v, "type")?;
    let channel = || ChannelId::new(s(v, "channel").unwrap_or_default());

    Some(match ty {
        "hello" => RtEvent::Hello,

        "message" => {
            let subtype = s(v, "subtype").unwrap_or("");
            match subtype {
                "message_changed" => {
                    let inner = v.get("message").cloned().unwrap_or(Value::Null);
                    RtEvent::MessageChanged {
                        channel: channel(),
                        ts: Ts::new(s(&inner, "ts").or_else(|| s(v, "ts")).unwrap_or_default()),
                        raw: inner,
                    }
                }
                "message_deleted" => RtEvent::MessageDeleted {
                    channel: channel(),
                    ts: Ts::new(
                        s(v, "deleted_ts")
                            .or_else(|| s(v, "ts"))
                            .unwrap_or_default(),
                    ),
                },
                // The parent's reply counters moved; the reply itself arrives
                // as its own `message` event.
                "message_replied" => {
                    let inner = v.get("message").cloned().unwrap_or(Value::Null);
                    RtEvent::ThreadReply {
                        channel: channel(),
                        thread_ts: Ts::new(s(&inner, "thread_ts").unwrap_or_default()),
                        raw: inner,
                    }
                }
                _ => RtEvent::Message {
                    channel: channel(),
                    ts: Ts::new(s(v, "ts")?),
                    raw: v.clone(),
                },
            }
        }

        "reaction_added" | "reaction_removed" => {
            let item = v.get("item")?;
            RtEvent::ReactionChanged {
                channel: ChannelId::new(s(item, "channel").unwrap_or_default()),
                ts: Ts::new(s(item, "ts").unwrap_or_default()),
                name: s(v, "reaction").unwrap_or_default().to_string(),
                user: UserId::new(s(v, "user").unwrap_or_default()),
                added: ty == "reaction_added",
            }
        }

        "channel_marked" | "im_marked" | "group_marked" | "mpim_marked" => RtEvent::Marked {
            channel: channel(),
            ts: Ts::new(s(v, "ts").unwrap_or_default()),
        },

        "thread_marked" => {
            let sub = v.get("subscription");
            RtEvent::ThreadMarked {
                channel: ChannelId::new(
                    sub.and_then(|x| s(x, "channel"))
                        .or_else(|| s(v, "channel"))
                        .unwrap_or_default(),
                ),
                thread_ts: Ts::new(sub.and_then(|x| s(x, "thread_ts")).unwrap_or_default()),
                ts: Ts::new(
                    sub.and_then(|x| s(x, "last_read"))
                        .or_else(|| s(v, "ts"))
                        .unwrap_or_default(),
                ),
            }
        }

        "desktop_notification" => RtEvent::DesktopNotification {
            channel: channel(),
            ts: Ts::new(s(v, "ts").or_else(|| s(v, "msg")).unwrap_or_default()),
            // `title` is the conversation, `subtitle` the workspace, and
            // `content` the message — Slack's own names, which do not read
            // like anybody else's.
            title: s(v, "title").unwrap_or_default().to_string(),
            text: s(v, "content").unwrap_or_default().to_string(),
            is_mention: v
                .get("is_channel_invite")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || s(v, "content").is_some_and(|c| c.contains('@')),
        },

        "user_typing" => RtEvent::Typing {
            channel: channel(),
            user: UserId::new(s(v, "user").unwrap_or_default()),
        },

        "presence_change" => {
            let presence = match s(v, "presence").unwrap_or("") {
                "active" => slk_core::Presence::Active,
                "away" => slk_core::Presence::Away,
                _ => slk_core::Presence::Unknown,
            };
            let users = match v.get("users").and_then(Value::as_array) {
                Some(a) => a
                    .iter()
                    .filter_map(|x| x.as_str().map(UserId::new))
                    .collect(),
                None => s(v, "user").map(UserId::new).into_iter().collect(),
            };
            RtEvent::PresenceChanged { users, presence }
        }

        "reconnect_url" => RtEvent::ReconnectUrl(s(v, "url").unwrap_or_default().to_string()),

        other => RtEvent::Unhandled(other.to_string()),
    })
}
