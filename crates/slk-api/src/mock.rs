//! A backend with no network behind it.
//!
//! This exists for one reason above all others: **every automated test drives
//! `--anonymous`, which selects this.** ytmtui learned the hard way that a UI
//! test typing into a signed-in instance eventually presses the wrong key and
//! writes to a real account. Here the equivalent is a message in someone's
//! employer's `#general`, so the mock makes that impossible rather than
//! unlikely.
//!
//! It also gives the client something to show in a demo and a screenshot.

use crate::backend::*;
use crate::error::{ErrorKind, Result, SlackError};
use crate::events::{EventStream, RtEvent};
use async_trait::async_trait;
use serde_json::json;
use slk_core::{
    ChannelId, Conversation, ConversationKind, Message, TeamId, Ts, User, UserId, Workspace,
};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::mpsc;

fn slk_api_error(id: &UserId) -> SlackError {
    SlackError::new("users.info", ErrorKind::NotFound, id.as_str())
}

pub struct MockBackend {
    team: TeamId,
    domain: String,
    self_id: UserId,
    /// Behind a lock because joining adds one, and a joined channel has to be
    /// in the next `boot` or the client shows an empty pane with no name --
    /// which is exactly the bug a mock that quietly succeeds would hide.
    convs: Mutex<Vec<Conversation>>,
    users: Vec<User>,
    messages: Mutex<HashMap<String, Vec<Message>>>,
    /// Where an uploaded file really is, so a download of it returns the bytes
    /// that were sent rather than a note saying it happened.
    uploaded: Mutex<HashMap<String, std::path::PathBuf>>,
    /// Seconds between the scripted messages the demo stream emits. Zero means
    /// no stream at all, which is what tests want.
    live_every: u64,
    /// How many sockets have been opened. The demo drops the first one after a
    /// few messages, because a websocket that drops is the normal state of a
    /// long-lived one and a client that cannot come back from it is broken in
    /// a way nothing else reveals.
    connects: std::sync::atomic::AtomicUsize,
    /// What this workspace has been told about its own user, so the interface
    /// can read back what it set.
    me_active: Mutex<bool>,
    me_status: Mutex<(String, String, i64)>,
    me_snoozed: Mutex<u32>,
    /// Conversations we have said we are typing in.
    typed: Mutex<Vec<ChannelId>>,
    /// When set, every send fails as a transport error. For the outbox.
    unreachable: std::sync::atomic::AtomicBool,
}

impl MockBackend {
    pub fn new() -> Self {
        Self::named("T0MOCK", "demo")
    }

    /// A second demo workspace, so the switcher has somewhere to switch to.
    /// Deliberately smaller and quieter than the first, because two identical
    /// ones would not show that they are separate.
    pub fn second() -> Self {
        let me = Self::named("T1MOCK", "other-corp");
        {
            let mut convs = me.convs.lock().unwrap();
            convs.retain(|c| !c.is_starred);
            convs.truncate(2);
            for c in convs.iter_mut() {
                c.unread = 0;
                c.mentions = 0;
                c.name = format!("{}-corp", c.name);
            }
        }
        me.messages.lock().unwrap().clear();
        me
    }

    pub fn named(team_id: &str, domain: &str) -> Self {
        let team = TeamId::new(team_id);
        let self_id = UserId::new("U0SELF");
        let mut me = MockBackend {
            team: team.clone(),
            domain: domain.to_string(),
            self_id: self_id.clone(),
            convs: Mutex::new(Vec::new()),
            me_active: Mutex::new(true),
            me_status: Mutex::new((String::new(), String::new(), 0)),
            me_snoozed: Mutex::new(0),
            typed: Mutex::new(Vec::new()),
            unreachable: std::sync::atomic::AtomicBool::new(false),
            users: Vec::new(),
            messages: Mutex::new(HashMap::new()),
            uploaded: Mutex::new(HashMap::new()),
            live_every: 0,
            connects: std::sync::atomic::AtomicUsize::new(0),
        };
        me.seed();
        me
    }

    /// Emit a scripted message every `secs` seconds, for `--demo`.
    /// Make sends fail as if the network were down, or stop.
    pub fn set_unreachable(&self, on: bool) {
        self.unreachable
            .store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn with_live_stream(mut self, secs: u64) -> Self {
        self.live_every = secs;
        self
    }

    /// Fill `#engineering` with `n` more messages cycling through every
    /// construct the renderer has to draw — bold, code, links, mentions,
    /// emoji, quotes, lists, code blocks, long prose, and every fiftieth one a
    /// Block Kit payload. For the interface to be measured against a
    /// conversation the size of a real one, not the size of a demo.
    ///
    /// They are dated before the seed, one second apart, so the demo's own
    /// story still ends the conversation.
    pub fn with_synthetic(self, n: usize) -> Self {
        let team = self.team.clone();
        let eng = ChannelId::new(format!("C0ENG{}", &team.as_str()[1..2]));
        let users = ["U0ALICE", "U0BOB", "U0CAROL", "U0SELF"];
        let corpus: [&str; 12] = [
            "plain sentence number {i}, nothing special about it",
            "*bold* and _italic_ and ~struck~ and `code` in one line ({i})",
            "see <https://example.com/issues/{i}|issue {i}> and <https://example.com/raw>",
            "cc <@U0ALICE> and <#C0ENG0|engineering> and <!here> ({i})",
            "shipped :rocket: :tada: :white_check_mark: ({i})",
            "> a quoted line ({i})\n> that continues\nand a reply after it",
            "• first item ({i})\n• second item\n• third item",
            "1. one ({i})\n2. two\n3. three",
            "```\nfn main() {{\n    println!(\"{i}\");\n}}\n```",
            "a longer message so that wrapping has something to do: the deploy of build {i} \
             went through staging, then canary, then the two regional clusters, and every \
             dashboard stayed green the whole way, which is the first time this quarter",
            "日本語のテキスト と emoji 👨‍👩‍👧‍👦 と ligatures ({i})",
            "&lt;not a tag&gt; &amp; escaped ({i})",
        ];
        let base: i64 = 1_725_600_000;
        let mut msgs = Vec::with_capacity(n);
        for i in 0..n {
            let ts = format!("{}.{:06}", base + i as i64, 100);
            let user = users[i % users.len()];
            let mut v = if i % 50 == 49 {
                json!({
                    "type": "message", "user": "U0BOB", "ts": ts,
                    "text": format!("Deploy summary #{i}"),
                    "blocks": [
                        {"type": "header", "text": {"type": "plain_text", "text": format!("Deploy #{i} finished")}},
                        {"type": "section",
                         "text": {"type": "mrkdwn", "text": "*Status:* green :white_check_mark:\nAll clusters healthy."},
                         "fields": [
                            {"type": "mrkdwn", "text": "*Duration*\n3m 12s"},
                            {"type": "mrkdwn", "text": "*Commit*\n`a1b2c3d`"},
                            {"type": "mrkdwn", "text": "*Clusters*\neu-1, eu-2, us-1"},
                            {"type": "mrkdwn", "text": "*Errors*\n0"}
                         ],
                         "accessory": {"type": "button", "text": {"type": "plain_text", "text": "Dashboard"},
                                       "url": "https://example.com/dash"}},
                        {"type": "context", "elements": [{"type": "mrkdwn", "text": "triggered by <@U0ALICE> · main"}]},
                        {"type": "divider"},
                        {"type": "actions", "elements": [
                            {"type": "button", "text": {"type": "plain_text", "text": "Open build"}, "url": format!("https://example.com/build/{i}"), "style": "primary"},
                            {"type": "button", "text": {"type": "plain_text", "text": "Rollback"}, "style": "danger"}
                        ]}
                    ]
                })
            } else {
                let text = corpus[i % corpus.len()].replace("{i}", &i.to_string());
                json!({"type": "message", "user": user, "ts": ts, "text": text})
            };
            if i % 7 == 6 {
                v["reactions"] = json!([{"name": "+1", "count": 2, "users": ["U0ALICE", "U0BOB"]}]);
            }
            if let Some(m) = Message::parse(&team, &eng, &self.self_id, &v) {
                msgs.push(m);
            }
        }
        {
            let mut all = self.messages.lock().unwrap();
            let entry = all.entry(eng.as_str().to_string()).or_default();
            msgs.append(entry);
            *entry = msgs;
        }
        self
    }

    /// The demo's own picture, written to a temp file and posted, so a
    /// demo without any file on disk still has an image to draw.
    pub fn with_demo_image(self) -> Self {
        let p = std::env::temp_dir().join(format!("slack-light-demo-{}.png", std::process::id()));
        if std::fs::write(&p, include_bytes!("../assets/demo.png")).is_ok() {
            self.with_image(&p)
        } else {
            self
        }
    }

    /// Post `path` as a picture into `#engineering`, so the demo has an
    /// image to draw. Goes through the same `upload` the client uses, which
    /// is what makes the download side real.
    pub fn with_image(self, path: &std::path::Path) -> Self {
        let team = self.team.clone();
        let eng = ChannelId::new(format!("C0ENG{}", &team.as_str()[1..2]));
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "picture.png".into());
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let url = format!("mock://{name}");
        self.uploaded
            .lock()
            .unwrap()
            .insert(url.clone(), path.to_path_buf());
        let v = json!({
            "type": "message", "subtype": "file_share", "user": "U0ALICE",
            "ts": "1725701800.000100", "text": "screenshot from the canary",
            "files": [{"id": "F0SPIKE", "name": name, "size": size, "mimetype": "image/png",
                       "url_private": url, "original_w": 320, "original_h": 200}],
        });
        if let Some(m) = Message::parse(&team, &eng, &self.self_id, &v) {
            let mut all = self.messages.lock().unwrap();
            let entry = all.entry(eng.as_str().to_string()).or_default();
            entry.push(m);
            entry.sort_by(|a, b| a.ts.cmp(&b.ts));
        }
        self
    }

    fn seed(&mut self) {
        let t = &self.team;
        let team_id = t.as_str().to_string();
        for (id, name, kind, unread, mentions, latest) in [
            (
                "C0ENG",
                "engineering",
                ConversationKind::Public,
                3u32,
                1u32,
                "1725701900.000100",
            ),
            (
                "C0GEN",
                "general",
                ConversationKind::Public,
                2,
                0,
                "1725701400.000100",
            ),
            (
                "C0LEAD",
                "leads",
                ConversationKind::Private,
                1,
                0,
                "1725700900.000100",
            ),
            (
                "C0DES",
                "design",
                ConversationKind::Public,
                0,
                0,
                "1725690000.000100",
            ),
        ] {
            self.convs.lock().unwrap().push(Conversation {
                team: t.clone(),
                id: ChannelId::new(format!("{id}{}", &team_id[1..2])),
                kind,
                name: name.into(),
                topic: if id == "C0ENG" {
                    "Deploys, incidents, on-call".into()
                } else {
                    String::new()
                },
                purpose: String::new(),
                is_member: true,
                is_archived: false,
                is_starred: id == "C0ENG",
                is_muted: false,
                is_shared: false,
                member_count: Some(42),
                last_read: Some(Ts::new("1725700000.000000")),
                latest: Some(Ts::new(latest)),
                unread,
                mentions,
                notify: Default::default(),
            });
        }
        self.convs.lock().unwrap().push(Conversation {
            team: t.clone(),
            id: ChannelId::new(format!("D0ALICE{}", &team_id[1..2])),
            kind: ConversationKind::Dm {
                peer: UserId::new("U0ALICE"),
            },
            name: "alice".into(),
            topic: String::new(),
            purpose: String::new(),
            is_member: true,
            is_archived: false,
            is_starred: false,
            is_muted: false,
            is_shared: false,
            member_count: Some(2),
            last_read: Some(Ts::new("1725700000.000000")),
            latest: Some(Ts::new("1725701000.000100")),
            unread: 1,
            mentions: 1,
            notify: Default::default(),
        });

        // Enough of a profile to be worth opening: a title, a timezone, and
        // one person with a status set.
        for (id, name, real, title, tz, status) in [
            ("U0SELF", "petr", "Petr Olivka", "", "Europe/Prague", ""),
            (
                "U0ALICE",
                "alice",
                "Alice Brennan",
                "Staff engineer",
                "Europe/London",
                "shipping v2.4",
            ),
            (
                "U0BOB",
                "bob",
                "Bob Ferreira",
                "SRE",
                "America/New_York",
                "",
            ),
            (
                "U0CAROL",
                "carol",
                "Carol Nkemdirim",
                "Engineering manager",
                "Africa/Lagos",
                "",
            ),
        ] {
            self.users.push(
                User::parse(
                    t,
                    &json!({"id": id, "name": name, "tz": tz, "profile": {
                        "display_name": name,
                        "real_name": real,
                        "title": title,
                        "status_text": status,
                        "status_emoji": if status.is_empty() { "" } else { ":rocket:" },
                    }}),
                )
                .expect("seed user"),
            );
        }

        let eng = ChannelId::new(format!("C0ENG{}", &team_id[1..2]));
        let script = [
            (
                "U0ALICE",
                "1725701100.000100",
                "morning — starting the v2.4 deploy now",
            ),
            (
                "U0BOB",
                "1725701200.000100",
                "*ack*. i'll watch the dashboards",
            ),
            (
                "U0ALICE",
                "1725701500.000100",
                "migration is running :hourglass_flowing_sand:",
            ),
            (
                "U0BOB",
                "1725701600.000100",
                "cc <@U0SELF> — the retry limit is `5` now, not 3",
            ),
            (
                "U0CAROL",
                "1725701700.000100",
                "> and re-run tomorrow if it stalls\nagreed",
            ),
            (
                "U0ALICE",
                "1725701900.000100",
                "Deploy of v2.4 finished :white_check_mark: — details in the thread",
            ),
        ];
        let mut msgs = Vec::new();
        for (user, ts, text) in script {
            let mut v = json!({"type":"message","user":user,"ts":ts,"text":text});
            // One pinned message, so the pinned view has something to show
            // in `--anonymous` and the suite can assert on it.
            if ts == "1725701600.000100" {
                v["pinned_to"] = json!(["C0ENG"]);
            }
            if ts == "1725701900.000100" {
                v["thread_ts"] = json!(ts);
                v["reply_count"] = json!(2);
                v["reply_users"] = json!(["U0BOB", "U0ALICE"]);
                v["latest_reply"] = json!("1725702000.000100");
                v["reactions"] = json!([
                    {"name":"tada","count":3,"users":["U0BOB"]},
                    {"name":"+1","count":1,"users":["U0SELF"]},
                    // One of the workspace's own, so the picture path is
                    // exercised rather than merely written.
                    {"name":"shipit","count":2,"users":["U0ALICE","U0CAROL"]}
                ]);
            }
            if let Some(m) = Message::parse(t, &eng, &self.self_id, &v) {
                msgs.push(m);
            }
        }
        // Two replies inside that thread.
        for (user, ts, text) in [
            ("U0BOB", "1725701950.000100", "was the migration included?"),
            ("U0ALICE", "1725702000.000100", "yes, it ran in step 3"),
        ] {
            let v = json!({"type":"message","user":user,"ts":ts,"text":text,
                           "thread_ts":"1725701900.000100"});
            if let Some(m) = Message::parse(t, &eng, &self.self_id, &v) {
                msgs.push(m);
            }
        }
        self.messages
            .lock()
            .unwrap()
            .insert(eng.as_str().to_string(), msgs);

        // A snippet, so the inline preview has something to draw and the
        // suite can assert on it. Shaped the way Slack sends one: a truncated
        // `preview`, the real line count, and a filetype.
        let snip = json!({
            "type": "message", "subtype": "file_share", "user": "U0BOB",
            "ts": "1725701650.000100", "text": "the retry config, for reference",
            "files": [{
                "id": "F0RETRY", "name": "retry.toml", "mimetype": "text/plain",
                "filetype": "toml", "size": 412, "lines": 21,
                "preview": "[retry]\nattempts = 5\nbackoff = \"exponential\"\nbase_ms = 250\nmax_ms = 30000\njitter = true\n\n[retry.per_endpoint]\n\"chat.postMessage\" = 3\n\"conversations.history\" = 5\n\"users.list\" = 2\n\"search.messages\" = 2\n\"files.upload\" = 1\n\"reactions.add\" = 4",
                "url_private": "mock://retry.toml"
            }],
        });
        if let Some(m) = Message::parse(t, &eng, &self.self_id, &snip) {
            let mut all = self.messages.lock().unwrap();
            let entry = all.entry(eng.as_str().to_string()).or_default();
            entry.push(m);
            entry.sort_by(|a, b| a.ts.cmp(&b.ts));
        }

        let dm = ChannelId::new(format!("D0ALICE{}", &team_id[1..2]));
        let v = json!({"type":"message","user":"U0ALICE","ts":"1725701000.000100",
                       "text":"can you look at the retry logic when you get a moment?"});
        if let Some(m) = Message::parse(t, &dm, &self.self_id, &v) {
            self.messages
                .lock()
                .unwrap()
                .insert(dm.as_str().to_string(), vec![m]);
        }
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// An 8×8 PNG, opaque. Small enough to inline, real enough to decode.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x08, 0x08, 0x02, 0x00, 0x00, 0x00, 0x4B, 0x6D, 0x29,
    0xDC, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0xF0, 0x9A, 0xF7, 0x1F,
    0x2B, 0x62, 0x18, 0x5A, 0x12, 0x00, 0x65, 0x49, 0x79, 0xC1, 0x48, 0x91, 0x8E, 0xD7, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[async_trait]
impl SlackBackend for MockBackend {
    fn team(&self) -> &TeamId {
        &self.team
    }
    fn self_id(&self) -> &UserId {
        &self.self_id
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::session()
    }

    async fn whoami(&self) -> Result<Workspace> {
        Ok(Workspace {
            id: self.team.clone(),
            name: self.domain.clone(),
            domain: self.domain.clone(),
            self_id: self.self_id.clone(),
        })
    }

    async fn boot(&self) -> Result<Boot> {
        let conversations = self.convs.lock().unwrap().clone();
        let muted = conversations
            .iter()
            .filter(|c| c.is_muted)
            .map(|c| c.id.clone())
            .collect();
        Ok(Boot {
            conversations,
            users: self.users.clone(),
            muted,
        })
    }

    async fn counts(&self) -> Result<Counts> {
        Ok(Counts {
            conversations: self
                .convs
                .lock()
                .unwrap()
                .iter()
                .map(|c| CountEntry {
                    id: c.id.clone(),
                    last_read: c.last_read.clone(),
                    latest: c.latest.clone(),
                    unread: c.unread,
                    mentions: c.mentions,
                    history_invalid: false,
                })
                .collect(),
        })
    }

    async fn history(&self, ch: &ChannelId, q: HistoryQuery) -> Result<Page> {
        let all = self
            .messages
            .lock()
            .unwrap()
            .get(ch.as_str())
            .cloned()
            .unwrap_or_default();
        let mut messages: Vec<Message> = all
            .into_iter()
            .filter(|m| !m.is_reply())
            .filter(|m| match &q.latest {
                Some(l) if q.inclusive => m.ts <= *l,
                Some(l) => m.ts < *l,
                None => true,
            })
            .filter(|m| match &q.oldest {
                Some(o) if q.inclusive => m.ts >= *o,
                Some(o) => m.ts > *o,
                None => true,
            })
            .collect();
        // A page, the way Slack sends one: the *newest* `limit` of what
        // matched, oldest first, and `has_more` when something was cut. The
        // mock used to hand over the whole channel at once, which meant
        // scrollback could not be exercised against it at all.
        let limit = q.limit.max(1) as usize;
        let has_more = messages.len() > limit;
        if has_more {
            messages.drain(..messages.len() - limit);
        }
        Ok(Page {
            has_more,
            cursor: None,
            messages,
        })
    }

    async fn replies(&self, ch: &ChannelId, thread: &Ts, _q: HistoryQuery) -> Result<Page> {
        let all = self
            .messages
            .lock()
            .unwrap()
            .get(ch.as_str())
            .cloned()
            .unwrap_or_default();
        Ok(Page {
            messages: all
                .into_iter()
                .filter(|m| m.ts == *thread || m.thread_ts.as_ref() == Some(thread))
                .collect(),
            has_more: false,
            cursor: None,
        })
    }

    async fn users(&self) -> Result<Vec<User>> {
        Ok(self.users.clone())
    }

    async fn members(&self, ch: &ChannelId) -> Result<Vec<UserId>> {
        // A direct message has exactly the two people in it; a channel, in the
        // demo, has everybody.
        let convs = self.convs.lock().unwrap();
        let conv = convs.iter().find(|c| c.id == *ch);
        Ok(match conv.map(|c| &c.kind) {
            Some(ConversationKind::Dm { peer }) => vec![self.self_id.clone(), peer.clone()],
            _ => self.users.iter().map(|u| u.id.clone()).collect(),
        })
    }

    async fn user_info(&self, id: &UserId) -> Result<User> {
        self.users
            .iter()
            .find(|u| u.id == *id)
            .cloned()
            .ok_or_else(|| slk_api_error(id))
    }

    async fn custom_emoji(&self) -> Result<Vec<(String, String)>> {
        Ok(vec![
            ("shipit".into(), "mock://emoji/shipit.png".into()),
            ("blob-wave".into(), "mock://emoji/blob-wave.png".into()),
        ])
    }

    async fn search_files(&self, query: &str, _count: u16) -> Result<Vec<FileHit>> {
        // Over the files actually attached to the demo's messages, so what
        // the search finds is what the conversation shows.
        let q = query.to_lowercase();
        let all = self.messages.lock().unwrap();
        let mut out = Vec::new();
        for (ch, msgs) in all.iter() {
            for m in msgs {
                for f in &m.files {
                    if q.is_empty() || f.name.to_lowercase().contains(&q) {
                        out.push(FileHit {
                            id: f.id.clone(),
                            name: f.name.clone(),
                            mimetype: f.mimetype.clone(),
                            size: f.size,
                            url_private: f.url_private.clone(),
                            channel: Some(ChannelId::new(ch.clone())),
                            channel_name: String::new(),
                            user: match &m.author {
                                slk_core::Author::User(u) => Some(u.clone()),
                                _ => None,
                            },
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    async fn search(&self, query: &str, count: u16) -> Result<Vec<SearchHit>> {
        let q = query.to_lowercase();
        let all = self.messages.lock().unwrap();
        let mut out = Vec::new();
        for (ch, msgs) in all.iter() {
            let name = self
                .convs
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.id.as_str() == ch)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            for m in msgs {
                if m.text.to_lowercase().contains(&q) {
                    out.push(SearchHit {
                        channel: ChannelId::new(ch.clone()),
                        channel_name: name.clone(),
                        ts: m.ts.clone(),
                        user: match &m.author {
                            slk_core::Author::User(u) => Some(u.clone()),
                            _ => None,
                        },
                        text: m.text.clone(),
                    });
                }
            }
        }
        out.sort_by(|a, b| b.ts.cmp(&a.ts));
        out.truncate(count as usize);
        Ok(out)
    }

    async fn post(
        &self,
        ch: &ChannelId,
        thread: Option<&Ts>,
        text: &str,
        _client_msg_id: &str,
        _broadcast: bool,
    ) -> Result<Ts> {
        // Pretend the network is down. There is no other way to exercise the
        // outbox: a real transport failure cannot be arranged in a test, and
        // a queue that has never been drained is a queue nobody has tested.
        if self.unreachable.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(SlackError::new(
                "chat.postMessage",
                ErrorKind::Transport,
                "the demo workspace is pretending to be unreachable",
            ));
        }
        // A channel this workspace does not have is a refusal, the way Slack
        // answers `channel_not_found`. Accepting it would make the demo
        // workspace more forgiving than the real one, which is the wrong
        // direction for a mock to be wrong in.
        if !self
            .convs
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.id.as_str() == ch.as_str())
        {
            return Err(SlackError::new(
                "chat.postMessage",
                ErrorKind::NotFound,
                "channel_not_found",
            ));
        }
        // A plausible timestamp, monotonic within the run so ordering holds.
        let ts = Ts::new(format!(
            "{}.000100",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(1_725_800_000)
        ));
        let mut v = json!({"type":"message","user": self.self_id.as_str(),
                           "ts": ts.as_str(), "text": text});
        if let Some(t) = thread {
            v["thread_ts"] = json!(t.as_str());
        }
        if let Some(m) = Message::parse(&self.team, ch, &self.self_id, &v) {
            self.messages
                .lock()
                .unwrap()
                .entry(ch.as_str().to_string())
                .or_default()
                .push(m);
        }
        Ok(ts)
    }

    async fn mark(&self, _ch: &ChannelId, _ts: &Ts) -> Result<()> {
        Ok(())
    }

    async fn react(&self, ch: &ChannelId, ts: &Ts, name: &str, on: bool) -> Result<()> {
        let mut all = self.messages.lock().unwrap();
        let Some(msgs) = all.get_mut(ch.as_str()) else {
            return Ok(());
        };
        let Some(m) = msgs.iter_mut().find(|m| m.ts == *ts) else {
            return Ok(());
        };
        match m.reactions.iter_mut().find(|r| r.name == name) {
            Some(r) if on => {
                r.count += 1;
                r.by_me = true;
            }
            Some(r) => {
                r.count = r.count.saturating_sub(1);
                r.by_me = false;
            }
            None if on => m.reactions.push(slk_core::model::Reaction {
                name: name.to_string(),
                count: 1,
                by_me: true,
            }),
            None => {}
        }
        m.reactions.retain(|r| r.count > 0);
        Ok(())
    }

    async fn edit(&self, ch: &ChannelId, ts: &Ts, text: &str) -> Result<()> {
        let mut all = self.messages.lock().unwrap();
        if let Some(msgs) = all.get_mut(ch.as_str()) {
            if let Some(m) = msgs.iter_mut().find(|m| m.ts == *ts) {
                m.text = text.to_string();
                m.body = slk_core::mrkdwn::parse(text);
                m.edited = true;
            }
        }
        Ok(())
    }

    async fn delete(&self, ch: &ChannelId, ts: &Ts) -> Result<()> {
        let mut all = self.messages.lock().unwrap();
        if let Some(msgs) = all.get_mut(ch.as_str()) {
            msgs.retain(|m| m.ts != *ts);
        }
        Ok(())
    }

    async fn permalink(&self, ch: &ChannelId, ts: &Ts) -> Result<String> {
        Ok(format!(
            "https://demo.slack.com/archives/{}/p{}",
            ch.as_str(),
            ts.as_str().replace('.', "")
        ))
    }

    async fn me_message(&self, ch: &ChannelId, text: &str) -> Result<()> {
        let ts = Ts::new(format!(
            "{}.000300",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ));
        let v = json!({"type": "message", "subtype": "me_message",
                       "user": self.self_id.as_str(), "ts": ts.as_str(), "text": text});
        if let Some(m) = slk_core::Message::parse(&self.team, ch, &self.self_id, &v) {
            self.messages
                .lock()
                .unwrap()
                .entry(ch.as_str().to_string())
                .or_default()
                .push(m);
        }
        Ok(())
    }

    // The demo keeps its own presence, status and snooze, so the interface
    // has something to read back. A "you are away" that never changes is
    // indistinguishable from one that is not wired up.
    async fn set_presence(&self, active: bool) -> Result<()> {
        *self.me_active.lock().unwrap() = active;
        Ok(())
    }
    async fn set_status(&self, text: &str, emoji: &str, expires: i64) -> Result<()> {
        *self.me_status.lock().unwrap() = (text.to_string(), emoji.to_string(), expires);
        Ok(())
    }
    async fn snooze(&self, minutes: u32) -> Result<()> {
        *self.me_snoozed.lock().unwrap() = minutes;
        Ok(())
    }
    async fn typing(&self, ch: &ChannelId) -> Result<()> {
        self.typed.lock().unwrap().push(ch.clone());
        Ok(())
    }
    async fn pins(&self, ch: &ChannelId) -> Result<Vec<Ts>> {
        Ok(self
            .messages
            .lock()
            .unwrap()
            .get(ch.as_str())
            .map(|ms| {
                ms.iter()
                    .filter(|m| m.pinned)
                    .map(|m| m.ts.clone())
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn channel_op(&self, op: ChannelOp) -> Result<()> {
        // The demo workspace answers these for real, because a star that does
        // not move is indistinguishable from a star that is not wired up, and
        // the a11y suite has to be able to tell the two apart.
        match &op {
            ChannelOp::Star(ch, on) => {
                let mut convs = self.convs.lock().unwrap();
                if let Some(c) = convs.iter_mut().find(|c| c.id == *ch) {
                    c.is_starred = *on;
                }
            }
            ChannelOp::SetMuted(all) => {
                let mut convs = self.convs.lock().unwrap();
                for c in convs.iter_mut() {
                    c.is_muted = all.contains(&c.id);
                }
            }
            ChannelOp::SetTopic(ch, t) => {
                let mut convs = self.convs.lock().unwrap();
                if let Some(c) = convs.iter_mut().find(|c| c.id == *ch) {
                    c.topic = t.clone();
                }
            }
            ChannelOp::SetPurpose(ch, t) => {
                let mut convs = self.convs.lock().unwrap();
                if let Some(c) = convs.iter_mut().find(|c| c.id == *ch) {
                    c.purpose = t.clone();
                }
            }
            ChannelOp::Leave(ch) => {
                let mut convs = self.convs.lock().unwrap();
                if let Some(c) = convs.iter_mut().find(|c| c.id == *ch) {
                    c.is_member = false;
                }
            }
            _ => {}
        }
        if let ChannelOp::Join(ch) = &op {
            let mut convs = self.convs.lock().unwrap();
            if !convs.iter().any(|c| c.id == *ch) {
                let name = if ch.as_str().ends_with('0') {
                    "random"
                } else {
                    "announcements"
                };
                convs.push(Conversation {
                    team: self.team.clone(),
                    id: ch.clone(),
                    kind: ConversationKind::Public,
                    name: name.into(),
                    topic: String::new(),
                    purpose: String::new(),
                    is_member: true,
                    is_archived: false,
                    is_starred: false,
                    is_muted: false,
                    is_shared: false,
                    member_count: Some(12),
                    last_read: None,
                    latest: None,
                    unread: 0,
                    mentions: 0,
                    notify: Default::default(),
                });
            }
        }
        Ok(())
    }
    async fn slash(&self, _ch: &ChannelId, command: &str, _text: &str) -> Result<()> {
        // The demo has no apps, so an unknown command is refused the way a
        // workspace without that app would refuse it.
        Err(SlackError::new(
            "chat.command",
            ErrorKind::NotFound,
            format!("no such command: {command}"),
        ))
    }
    async fn save(&self, ch: &ChannelId, ts: &Ts, on: bool) -> Result<()> {
        let mut all = self.messages.lock().unwrap();
        if let Some(msgs) = all.get_mut(ch.as_str()) {
            if let Some(m) = msgs.iter_mut().find(|m| m.ts == *ts) {
                m.saved = on;
            }
        }
        Ok(())
    }
    async fn usergroups(&self) -> Result<Vec<(String, String)>> {
        Ok(vec![("S0DESIGN".into(), "design-team".into())])
    }

    async fn public_channels(&self, _limit: u16) -> Result<Vec<Conversation>> {
        // Two the demo user is not in, so the picker has something to join.
        Ok(["random", "announcements"]
            .iter()
            .enumerate()
            .map(|(i, name)| Conversation {
                team: self.team.clone(),
                id: ChannelId::new(format!("C0BROWSE{i}")),
                kind: ConversationKind::Public,
                name: (*name).to_string(),
                topic: String::new(),
                purpose: String::new(),
                is_member: false,
                is_archived: false,
                is_starred: false,
                is_muted: false,
                is_shared: false,
                member_count: Some(12 + i as u32),
                last_read: None,
                latest: None,
                unread: 0,
                mentions: 0,
                notify: Default::default(),
            })
            .collect())
    }

    async fn follow_thread(&self, ch: &ChannelId, thread: &Ts, on: bool) -> Result<()> {
        let mut all = self.messages.lock().unwrap();
        if let Some(msgs) = all.get_mut(ch.as_str()) {
            if let Some(m) = msgs.iter_mut().find(|m| m.ts == *thread) {
                m.subscribed = on;
            }
        }
        Ok(())
    }

    async fn pin(&self, ch: &ChannelId, ts: &Ts, on: bool) -> Result<()> {
        let mut all = self.messages.lock().unwrap();
        if let Some(msgs) = all.get_mut(ch.as_str()) {
            if let Some(m) = msgs.iter_mut().find(|m| m.ts == *ts) {
                m.pinned = on;
            }
        }
        Ok(())
    }

    async fn download(&self, url: &str, to: &std::path::Path) -> Result<u64> {
        if let Some(d) = to.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        // The demo's custom emoji: a real PNG, because the path under test
        // ends in `Texture::from_file`, and a text file with a `.png` name
        // would make the decode fail rather than the feature work.
        if url.starts_with("mock://emoji/") {
            std::fs::write(to, TINY_PNG)
                .map_err(|e| SlackError::new("download", ErrorKind::Transport, e.to_string()))?;
            return Ok(TINY_PNG.len() as u64);
        }
        // A file this workspace was given comes back as itself. Handing back a
        // note saying a download happened would make every path that reads the
        // bytes — decoding a picture, most of all — untestable.
        let source = self.uploaded.lock().unwrap().get(url).cloned();
        let body = match source {
            Some(p) => std::fs::read(&p)
                .map_err(|e| SlackError::new("download", ErrorKind::Transport, e.to_string()))?,
            None => format!("demo file from {url}\n").into_bytes(),
        };
        std::fs::write(to, &body)
            .map_err(|e| SlackError::new("download", ErrorKind::Transport, e.to_string()))?;
        Ok(body.len() as u64)
    }

    async fn upload(
        &self,
        ch: &ChannelId,
        thread: Option<&Ts>,
        path: &std::path::Path,
        comment: Option<&str>,
    ) -> Result<()> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let mock_url = format!("mock://{name}");
        self.uploaded
            .lock()
            .unwrap()
            .insert(mock_url.clone(), path.to_path_buf());
        let mime = match path.extension().and_then(|e| e.to_str()) {
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "application/octet-stream",
        };
        let ts = Ts::new(format!(
            "{}.000200",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ));
        let v = json!({
            "type": "message", "subtype": "file_share",
            "user": self.self_id.as_str(), "ts": ts.as_str(),
            "text": comment.unwrap_or(""),
            "thread_ts": thread.map(|t| t.as_str()),
            "files": [{"id": format!("F{}", ts.as_str()), "name": name, "size": size,
                       "mimetype": mime, "url_private": mock_url}],
        });
        if let Some(m) = slk_core::Message::parse(&self.team, ch, &self.self_id, &v) {
            self.messages
                .lock()
                .unwrap()
                .entry(ch.as_str().to_string())
                .or_default()
                .push(m);
        }
        Ok(())
    }

    async fn connect(&self, _presence: &[UserId]) -> Result<Option<EventStream>> {
        let (tx, rx) = mpsc::channel(64);
        let every = self.live_every;
        let first = self
            .connects
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            == 0;
        let team = self.team.clone();
        let eng = format!("C0ENG{}", &self.team.as_str()[1..2]);
        // The demo shows presence because a client without it looks broken,
        // not because the mock knows anything.
        let peers: Vec<UserId> = self
            .convs
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| match &c.kind {
                ConversationKind::Dm { peer } => Some(peer.clone()),
                _ => None,
            })
            .collect();

        tokio::spawn(async move {
            let _ = tx.send(RtEvent::Hello).await;
            if !peers.is_empty() {
                let _ = tx
                    .send(RtEvent::PresenceChanged {
                        users: peers,
                        presence: slk_core::Presence::Active,
                    })
                    .await;
            }
            if every == 0 {
                // Tests want a stream that stays open and says nothing, so the
                // engine's reconnect path is not exercised by accident.
                std::future::pending::<()>().await;
                return;
            }
            let lines = [
                ("U0BOB", "one more thing — the dashboards look clean"),
                ("U0ALICE", "nice :tada:"),
                (
                    "U0CAROL",
                    "cc <@U0SELF> can you sanity-check the rollback plan?",
                ),
            ];
            let mut i = 0usize;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(every)).await;
                // Drop the first socket partway through, once.
                if first && i == 3 {
                    return;
                }
                let (user, text) = lines[i % lines.len()];
                i += 1;
                // Somebody types before they say something, the way people
                // do. Without this the received indicator has nothing to
                // draw and cannot be tested at all.
                let _ = tx
                    .send(RtEvent::Typing {
                        channel: ChannelId::new(eng.clone()),
                        user: UserId::new(user),
                    })
                    .await;
                tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                let ts = format!(
                    "{}.000100",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                );
                let raw = json!({"type":"message","user":user,"ts":ts,
                                 "text":text,"channel":eng,"team":team.as_str()});
                if tx
                    .send(RtEvent::Message {
                        channel: ChannelId::new(eng.clone()),
                        ts: Ts::new(ts),
                        raw,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        Ok(Some(rx))
    }
}
