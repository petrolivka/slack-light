//! The entities the client shows, parsed defensively from Slack's JSON.
//!
//! Every accessor returns an `Option` and every unknown shape degrades. That is
//! not defensive style for its own sake: half of what this client reads is
//! undocumented, "the shape is wrong" is an expected condition rather than an
//! exceptional one, and a panic takes the whole client down mid-conversation
//! where an empty pane does not.

use crate::blocks::{self, Attachment, Block};
use crate::ids::*;
use crate::Doc;
use serde_json::Value;

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str()
}
fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}
fn u(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: TeamId,
    pub name: String,
    pub domain: String,
    pub self_id: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationKind {
    Public,
    Private,
    Dm { peer: UserId },
    Mpim { members: Vec<UserId> },
}

impl ConversationKind {
    pub fn is_dm(&self) -> bool {
        matches!(
            self,
            ConversationKind::Dm { .. } | ConversationKind::Mpim { .. }
        )
    }
}

/// What a conversation's notifications are set to. `Default` means "whatever
/// the workspace says", which the client cannot compute and does not try to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotifyPref {
    #[default]
    Default,
    All,
    Mentions,
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub team: TeamId,
    pub id: ChannelId,
    pub kind: ConversationKind,
    pub name: String,
    pub topic: String,
    pub purpose: String,
    pub is_member: bool,
    pub is_archived: bool,
    pub is_starred: bool,
    pub is_muted: bool,
    pub is_shared: bool,
    pub member_count: Option<u32>,
    /// Where this user has read to. The anchor of the whole unread model.
    pub last_read: Option<Ts>,
    /// The newest message the server knows about.
    pub latest: Option<Ts>,
    pub unread: u32,
    pub mentions: u32,
    pub notify: NotifyPref,
}

impl Conversation {
    pub fn parse(team: &TeamId, v: &Value) -> Option<Self> {
        let id = ChannelId::from(s(v, "id")?);
        // `client.userBoot` groups conversations into `channels`, `ims` and
        // `mpims` arrays and does not always repeat the `is_im` flag on the
        // objects inside them. The id prefix does not lie: D is a direct
        // message, G a group, C a channel. Relying on the flag alone stored
        // every DM as a public channel with no name.
        let by_prefix = id.as_str().chars().next().unwrap_or('C');
        let is_im = b(v, "is_im") || by_prefix == 'D';
        let is_mpim = b(v, "is_mpim") || (by_prefix == 'G' && v.get("members").is_some());
        let kind = if is_im {
            ConversationKind::Dm {
                peer: UserId::from(s(v, "user").unwrap_or("")),
            }
        } else if is_mpim {
            ConversationKind::Mpim {
                members: arr(v, "members")
                    .iter()
                    .filter_map(|m| m.as_str().map(UserId::from))
                    .collect(),
            }
        } else if b(v, "is_private") || b(v, "is_group") || by_prefix == 'G' {
            ConversationKind::Private
        } else {
            ConversationKind::Public
        };

        Some(Conversation {
            team: team.clone(),
            id,
            kind,
            name: s(v, "name")
                .or_else(|| s(v, "name_normalized"))
                .unwrap_or("")
                .to_string(),
            topic: v
                .get("topic")
                .and_then(|t| s(t, "value"))
                .unwrap_or("")
                .to_string(),
            purpose: v
                .get("purpose")
                .and_then(|p| s(p, "value"))
                .unwrap_or("")
                .to_string(),
            // A DM is never "joined", but it is always ours to read. And a
            // conversation that came back from `client.userBoot` is one the
            // user is in, whether or not the flag says so.
            is_member: b(v, "is_member") || is_im || is_mpim || v.get("is_member").is_none(),
            is_archived: b(v, "is_archived"),
            is_starred: b(v, "is_starred"),
            is_muted: false, // comes from prefs, not from the channel object
            is_shared: b(v, "is_shared") || b(v, "is_ext_shared"),
            member_count: v
                .get("num_members")
                .and_then(Value::as_u64)
                .map(|n| n as u32),
            last_read: s(v, "last_read").map(Ts::from),
            latest: s(v, "latest")
                .map(Ts::from)
                .or_else(|| v.get("latest").and_then(|l| s(l, "ts")).map(Ts::from)),
            unread: u(v, "unread_count_display").max(u(v, "unread_count")) as u32,
            mentions: u(v, "mention_count_display").max(u(v, "mention_count")) as u32,
            notify: NotifyPref::Default,
        })
    }

    /// The label the sidebar shows. A DM has no name of its own, so the caller
    /// substitutes the peer's.
    pub fn display_name(&self) -> &str {
        &self.name
    }

    pub fn has_unread(&self) -> bool {
        self.unread > 0 || self.mentions > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presence {
    Active,
    #[default]
    Unknown,
    Away,
    Dnd,
    Deactivated,
}

impl Presence {
    pub fn glyph(self) -> &'static str {
        match self {
            Presence::Active => "●",
            Presence::Away => "○",
            Presence::Dnd => "◐",
            Presence::Deactivated => "⊘",
            Presence::Unknown => " ",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserStatus {
    pub text: String,
    pub emoji: String,
    pub expires: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub team: TeamId,
    pub id: UserId,
    pub name: String,
    pub display_name: String,
    pub real_name: String,
    pub title: String,
    pub tz: Option<String>,
    pub is_bot: bool,
    pub is_deleted: bool,
    pub is_external: bool,
    pub status: UserStatus,
    pub presence: Presence,
}

impl User {
    pub fn parse(team: &TeamId, v: &Value) -> Option<Self> {
        let id = UserId::from(s(v, "id")?);
        let profile = v.get("profile");
        let get = |k: &str| profile.and_then(|p| s(p, k)).unwrap_or("").to_string();
        let display_name = {
            let d = get("display_name");
            if d.is_empty() {
                s(v, "name").unwrap_or(id.as_str()).to_string()
            } else {
                d
            }
        };
        Some(User {
            team: team.clone(),
            id,
            name: s(v, "name").unwrap_or("").to_string(),
            display_name,
            real_name: get("real_name"),
            title: get("title"),
            tz: s(v, "tz").map(str::to_string),
            is_bot: b(v, "is_bot"),
            is_deleted: b(v, "deleted"),
            is_external: b(v, "is_stranger") || b(v, "is_restricted"),
            status: UserStatus {
                text: get("status_text"),
                emoji: get("status_emoji").trim_matches(':').to_string(),
                expires: profile
                    .and_then(|p| p.get("status_expiration"))
                    .and_then(Value::as_i64)
                    .filter(|n| *n > 0),
            },
            presence: if b(v, "deleted") {
                Presence::Deactivated
            } else {
                Presence::Unknown
            },
        })
    }

    /// What to show in a message header or a member list.
    pub fn label(&self) -> &str {
        if !self.display_name.is_empty() {
            &self.display_name
        } else if !self.real_name.is_empty() {
            &self.real_name
        } else {
            &self.name
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Author {
    User(UserId),
    Bot { id: BotId, name: String },
    System,
}

impl Author {
    pub fn id_str(&self) -> &str {
        match self {
            Author::User(u) => u.as_str(),
            Author::Bot { id, .. } => id.as_str(),
            Author::System => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    pub id: FileId,
    pub name: String,
    pub mimetype: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub url_private: Option<String>,
}

impl FileMeta {
    pub fn is_image(&self) -> bool {
        self.mimetype.starts_with("image/")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub name: String,
    pub count: u32,
    pub by_me: bool,
}

/// Where an outgoing message is in its life. The UI shows all three states,
/// because a message that silently failed to send is the worst outcome a chat
/// client can produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    Confirmed,
    /// Written locally and rendered immediately; the server has not answered.
    Pending(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub team: TeamId,
    pub channel: ChannelId,
    pub ts: Ts,
    pub thread_ts: Option<Ts>,
    pub author: Author,
    /// Present for bot messages, which carry their name inline rather than in
    /// the user directory.
    pub author_name: Option<String>,
    pub subtype: Option<String>,
    /// The raw mrkdwn as sent. Kept as the fallback and for search.
    pub text: String,
    pub body: Doc,
    pub blocks: Vec<Block>,
    pub attachments: Vec<Attachment>,
    pub files: Vec<FileMeta>,
    pub reactions: Vec<Reaction>,
    pub edited: bool,
    pub deleted: bool,
    pub reply_count: u32,
    pub reply_users: Vec<UserId>,
    pub latest_reply: Option<Ts>,
    pub subscribed: bool,
    pub pinned: bool,
    pub saved: bool,
    pub delivery: Delivery,
    /// Exactly what Slack sent, kept so the store can hold it verbatim.
    ///
    /// The architecture's first decision: a parser fix then becomes a
    /// migration over data already held rather than a re-fetch of everyone's
    /// history. It is also what stops a restart from showing messages stripped
    /// of their files, reactions and Block Kit, which is what happened when
    /// the engine wrote a summary instead.
    pub raw: String,
}

impl Message {
    pub fn parse(team: &TeamId, channel: &ChannelId, self_id: &UserId, v: &Value) -> Option<Self> {
        let ts = Ts::from(s(v, "ts")?);
        let subtype = s(v, "subtype").map(str::to_string);
        let author = match (s(v, "user"), s(v, "bot_id")) {
            (Some(u), _) if !u.is_empty() => Author::User(UserId::from(u)),
            (_, Some(bot)) => Author::Bot {
                id: BotId::from(bot),
                name: s(v, "username").unwrap_or("bot").to_string(),
            },
            _ => Author::System,
        };

        // Only rich_text folds into the body; the rest stay Block Kit so a
        // bot's layout survives.
        let body = crate::richtext::parse_message(v);
        let other: Vec<Value> = arr(v, "blocks")
            .iter()
            .filter(|x| s(x, "type") != Some("rich_text"))
            .cloned()
            .collect();

        Some(Message {
            team: team.clone(),
            channel: channel.clone(),
            ts,
            thread_ts: s(v, "thread_ts").map(Ts::from),
            author,
            author_name: s(v, "username").map(str::to_string),
            subtype,
            text: s(v, "text").unwrap_or("").to_string(),
            body,
            blocks: blocks::parse(&other),
            attachments: blocks::parse_attachments(arr(v, "attachments")),
            files: arr(v, "files")
                .iter()
                .filter_map(|f| {
                    Some(FileMeta {
                        id: FileId::from(s(f, "id")?),
                        name: s(f, "name").unwrap_or("file").to_string(),
                        mimetype: s(f, "mimetype")
                            .unwrap_or("application/octet-stream")
                            .to_string(),
                        size: u(f, "size"),
                        width: v
                            .get("original_w")
                            .and_then(Value::as_u64)
                            .map(|n| n as u32),
                        height: v
                            .get("original_h")
                            .and_then(Value::as_u64)
                            .map(|n| n as u32),
                        url_private: s(f, "url_private").map(str::to_string),
                    })
                })
                .collect(),
            reactions: arr(v, "reactions")
                .iter()
                .filter_map(|r| {
                    let name = s(r, "name")?;
                    Some(Reaction {
                        name: name.to_string(),
                        count: u(r, "count") as u32,
                        by_me: arr(r, "users")
                            .iter()
                            .any(|x| x.as_str() == Some(self_id.as_str())),
                    })
                })
                .collect(),
            edited: v.get("edited").is_some(),
            deleted: false,
            reply_count: u(v, "reply_count") as u32,
            reply_users: arr(v, "reply_users")
                .iter()
                .filter_map(|x| x.as_str().map(UserId::from))
                .collect(),
            latest_reply: s(v, "latest_reply").map(Ts::from),
            subscribed: b(v, "subscribed"),
            pinned: v.get("pinned_to").is_some(),
            saved: b(v, "is_starred"),
            delivery: Delivery::Confirmed,
            raw: v.to_string(),
        })
    }

    /// A message that only reports something happening in the channel. Slack
    /// renders these as one dim line and so does the client.
    pub fn is_system(&self) -> bool {
        matches!(
            self.subtype.as_deref(),
            Some(
                "channel_join"
                    | "channel_leave"
                    | "channel_topic"
                    | "channel_purpose"
                    | "channel_name"
                    | "channel_archive"
                    | "channel_unarchive"
                    | "group_join"
                    | "group_leave"
                    | "pinned_item"
            )
        )
    }

    /// Is this the parent of a thread, rather than a reply inside one?
    pub fn is_thread_parent(&self) -> bool {
        self.thread_ts.as_ref() == Some(&self.ts)
    }

    pub fn is_reply(&self) -> bool {
        matches!(&self.thread_ts, Some(t) if *t != self.ts)
    }

    pub fn mentions(&self, id: &UserId) -> bool {
        use crate::ast::{BlockNode, Inline};
        fn scan(xs: &[Inline], id: &UserId) -> bool {
            xs.iter().any(|x| match x {
                Inline::User { id: u, .. } => u == id.as_str(),
                Inline::Broadcast(_) => true,
                _ => false,
            })
        }
        self.body.0.iter().any(|n| match n {
            BlockNode::Section(xs) | BlockNode::Quote(xs) => scan(xs, id),
            BlockNode::List { items, .. } => items.iter().any(|i| scan(i, id)),
            BlockNode::Preformatted(_) => false,
        })
    }
}
