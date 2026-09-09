//! A read-only tour of every endpoint, and an inventory of what Slack sent
//! that we do not render.
//!
//! Two jobs, both about the same risk. Half of what this client reads is
//! undocumented, so the first job is to check that each endpoint still answers
//! and still carries the fields the client depends on. The second is to walk
//! the response and list every `type` discriminator against the set the
//! renderer has a branch for — what Slack sends and we ignore today is what
//! shows as an empty pane in a year.
//!
//!     cargo run --features dev-tools --bin probe
//!
//! Read-only: it calls nothing that writes. Point it only at a workspace you
//! created for the purpose.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use slk_core::{ChannelId, Message, TeamId, UserId};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// What the renderer has a real branch for. Anything outside these reaches a
/// placeholder at best.
const KNOWN_BLOCKS: &[&str] = &[
    "header",
    "section",
    "context",
    "divider",
    "image",
    "actions",
    "rich_text",
];
const KNOWN_RT_BLOCKS: &[&str] = &[
    "rich_text_section",
    "rich_text_list",
    "rich_text_preformatted",
    "rich_text_quote",
];
const KNOWN_RT_INLINE: &[&str] = &[
    "text",
    "link",
    "user",
    "usergroup",
    "channel",
    "broadcast",
    "emoji",
    "date",
    "color",
];
const KNOWN_BLOCK_ELEMENTS: &[&str] = &["button", "image", "mrkdwn", "plain_text"];
const KNOWN_TEXT_OBJECTS: &[&str] = &["mrkdwn", "plain_text"];

/// Where in the document a `type` was found.
///
/// `elements` means three different things depending on what holds it, and a
/// walker that lumps them together reports buttons and text objects as
/// unhandled rich-text inlines. This exists to find real gaps, so a false
/// positive costs more than a missing one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    Message,
    Block,
    RtBlock,
    RtInline,
    BlockElement,
    TextObject,
    Attachment,
    File,
    Reaction,
}

impl Slot {
    fn name(self) -> &'static str {
        match self {
            Slot::Message => "message",
            Slot::Block => "block",
            Slot::RtBlock => "rich_text block",
            Slot::RtInline => "rich_text inline",
            Slot::BlockElement => "block element",
            Slot::TextObject => "text object",
            Slot::Attachment => "attachment",
            Slot::File => "file",
            Slot::Reaction => "reaction",
        }
    }
    fn known(self) -> Option<&'static [&'static str]> {
        match self {
            Slot::Block => Some(KNOWN_BLOCKS),
            Slot::RtBlock => Some(KNOWN_RT_BLOCKS),
            Slot::RtInline => Some(KNOWN_RT_INLINE),
            Slot::BlockElement => Some(KNOWN_BLOCK_ELEMENTS),
            Slot::TextObject => Some(KNOWN_TEXT_OBJECTS),
            _ => None,
        }
    }
}

fn collect(v: &Value, slot: Slot, out: &mut BTreeMap<Slot, BTreeSet<String>>) {
    match v {
        Value::Object(m) => {
            let ty = m.get("type").and_then(Value::as_str);
            if let Some(t) = ty {
                out.entry(slot).or_default().insert(t.to_string());
            }
            for (k, x) in m {
                let next = match k.as_str() {
                    "blocks" => Slot::Block,
                    "elements" => match (slot, ty) {
                        (_, Some("rich_text")) => Slot::RtBlock,
                        (Slot::RtBlock | Slot::RtInline, _) => Slot::RtInline,
                        _ => Slot::BlockElement,
                    },
                    "text" | "title" | "label" if x.is_object() => Slot::TextObject,
                    "fields" => Slot::TextObject,
                    "accessory" => Slot::BlockElement,
                    "attachments" => Slot::Attachment,
                    "files" => Slot::File,
                    "reactions" => Slot::Reaction,
                    _ => slot,
                };
                collect(x, next, out);
            }
        }
        Value::Array(xs) => xs.iter().for_each(|x| collect(x, slot, out)),
        _ => {}
    }
}

struct Api {
    http: reqwest::Client,
    base: String,
    token: String,
    cookie: String,
}

impl Api {
    async fn call(&self, method: &str, form: &[(&str, &str)]) -> Result<(Value, u128)> {
        let t = Instant::now();
        let text = self
            .http
            .post(format!("{}/{method}", self.base))
            .header("Cookie", format!("d={}", self.cookie))
            .bearer_auth(&self.token)
            .form(form)
            .send()
            .await?
            .text()
            .await?;
        let ms = t.elapsed().as_millis();
        let v: Value = serde_json::from_str(&text)
            .with_context(|| format!("{method}: not JSON: {}", &text[..text.len().min(160)]))?;
        Ok((v, ms))
    }
}

fn n(v: &Value, key: &str) -> usize {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

#[tokio::main]
async fn main() -> Result<()> {
    let account = slack_light::auth::load()?;
    let api = Api {
        http: reqwest::Client::builder()
            .user_agent(concat!("slack-light/", env!("CARGO_PKG_VERSION"), " probe"))
            .gzip(true)
            .timeout(std::time::Duration::from_secs(30))
            .build()?,
        base: format!("https://{}.slack.com/api", account.team),
        token: account.token,
        cookie: account.cookie,
    };

    println!(
        "slack-light probe — read-only\n  workspace {}\n",
        account.team
    );

    let (auth, ms) = api.call("auth.test", &[]).await?;
    if auth.get("ok").and_then(Value::as_bool) != Some(true) {
        bail!(
            "auth.test failed: {}",
            auth.get("error").and_then(Value::as_str).unwrap_or("?")
        );
    }
    let team = TeamId::new(auth["team_id"].as_str().unwrap_or_default());
    let self_id = UserId::new(auth["user_id"].as_str().unwrap_or_default());
    println!(
        "  {:<26} ok {ms:>5} ms   {} / {}",
        "auth.test", auth["team"], auth["user"]
    );

    // Pick a channel to read: the first one boot returns.
    let (boot, ms) = api
        .call("client.userBoot", &[("_x_reason", "initial-data")])
        .await?;
    println!(
        "  {:<26} ok {ms:>5} ms   channels={} ims={}",
        "client.userBoot",
        n(&boot, "channels"),
        n(&boot, "ims")
    );
    let channel = std::env::args().nth(1).or_else(|| {
        boot.get("channels")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|c| c.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let Some(channel) = channel else {
        bail!("no channel to read; pass one as an argument");
    };

    // Everything else, with the fact that matters rather than the payload:
    // these responses are full of real names and message text.
    let mut history = Value::Null;
    for (method, form, note) in [
        (
            "client.counts",
            vec![("thread_counts_by_channel", "true")],
            "counts",
        ),
        (
            "conversations.history",
            vec![("channel", channel.as_str()), ("limit", "50")],
            "messages",
        ),
        ("users.list", vec![("limit", "50")], "members"),
        ("emoji.list", vec![], "emoji"),
        ("drafts.list", vec![], "drafts"),
        ("saved.list", vec![], "items"),
        ("users.prefs.get", vec![], "prefs"),
        ("dnd.info", vec![], "dnd"),
        (
            "bookmarks.list",
            vec![("channel_id", channel.as_str())],
            "bookmarks",
        ),
    ] {
        match api.call(method, &form).await {
            Ok((v, ms)) if v.get("ok").and_then(Value::as_bool) == Some(true) => {
                let count = match v.get(note) {
                    Some(Value::Array(a)) => format!("{} items", a.len()),
                    Some(Value::Object(o)) => format!("{} keys", o.len()),
                    _ => "ok".to_string(),
                };
                println!("  {method:<26} ok {ms:>5} ms   {count}");
                if method == "conversations.history" {
                    history = v;
                }
            }
            // `unknown_method` here is a finding about what this route can do,
            // not a failure of the probe.
            Ok((v, ms)) => println!(
                "  {method:<26} -- {ms:>5} ms   {}",
                v.get("error").and_then(Value::as_str).unwrap_or("?")
            ),
            Err(e) => println!("  {method:<26} ERR              {e}"),
        }
    }

    // ---- the inventory ----
    let msgs = history
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    println!("\ninventory over {} live messages", msgs.len());

    let mut inv: BTreeMap<Slot, BTreeSet<String>> = BTreeMap::new();
    for m in &msgs {
        collect(m, Slot::Message, &mut inv);
    }
    let mut unhandled = Vec::new();
    for (slot, types) in &inv {
        println!(
            "  {:<18} {}",
            slot.name(),
            types.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        if let Some(known) = slot.known() {
            for t in types {
                if !known.contains(&t.as_str()) {
                    unhandled.push(format!("{}: {t}", slot.name()));
                }
            }
        }
    }
    if unhandled.is_empty() {
        println!("\n  every block and element type here has a renderer");
    } else {
        println!("\n  NOT HANDLED: {}", unhandled.join(", "));
    }

    // ---- and the parsers over the same data ----
    let ch = ChannelId::new(&channel);
    let parsed: Vec<Message> = msgs
        .iter()
        .filter_map(|m| Message::parse(&team, &ch, &self_id, m))
        .collect();
    println!(
        "\n  parsed {}/{} messages, {} with rich_text blocks, {} with reactions, {} threaded",
        parsed.len(),
        msgs.len(),
        msgs.iter().filter(|m| m.get("blocks").is_some()).count(),
        parsed.iter().filter(|m| !m.reactions.is_empty()).count(),
        parsed.iter().filter(|m| m.reply_count > 0).count()
    );
    if parsed.len() != msgs.len() {
        println!("  ⚠ {} messages did not parse", msgs.len() - parsed.len());
    }

    Ok(())
}
