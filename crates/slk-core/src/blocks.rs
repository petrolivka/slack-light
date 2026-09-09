//! Block Kit, reduced to what a terminal can honestly show.
//!
//! Apps post these: a deploy summary from CI, a Jira card, a poll. The client
//! renders the documented subset and labels the rest, because a bot message
//! that says `[unsupported block: video]` is readable and one that renders
//! nothing is a bug report.

use crate::ast::Doc;
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

/// A Block Kit text object: `{type: "mrkdwn"|"plain_text", text: "..."}`.
fn text_object(v: &Value) -> Option<Doc> {
    let t = s(v, "text")?;
    Some(match s(v, "type") {
        Some("mrkdwn") => crate::mrkdwn::parse(t),
        // plain_text is literal: no `*bold*`, no `<@U1>`, but emoji shortcodes
        // are still substituted when `emoji` is true (the default).
        _ => Doc(vec![crate::ast::BlockNode::Section(vec![
            crate::ast::Inline::Text {
                text: t.to_string(),
                style: Default::default(),
            },
        ])]),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub url: Option<String>,
    pub style: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Header(String),
    Section {
        text: Option<Doc>,
        fields: Vec<Doc>,
        accessory: Option<Accessory>,
    },
    Context(Vec<Doc>),
    Divider,
    Image {
        url: String,
        alt: String,
        title: Option<String>,
    },
    Actions(Vec<Button>),
    RichText(Doc),
    /// Something we do not render. Carries the type so the placeholder can say
    /// which, which is the difference between a bug report and a shrug.
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accessory {
    Image { url: String, alt: String },
    Button(Button),
    Other(String),
}

fn button(v: &Value) -> Button {
    Button {
        label: v
            .get("text")
            .and_then(|t| s(t, "text"))
            .unwrap_or("button")
            .to_string(),
        url: s(v, "url").map(str::to_string),
        style: s(v, "style").map(str::to_string),
    }
}

pub fn parse(blocks: &[Value]) -> Vec<Block> {
    let mut out = Vec::new();
    for b in blocks {
        let ty = s(b, "type").unwrap_or("");
        out.push(match ty {
            "header" => Block::Header(
                b.get("text")
                    .and_then(|t| s(t, "text"))
                    .unwrap_or("")
                    .to_string(),
            ),
            "section" => {
                let text = b.get("text").and_then(text_object);
                let fields = arr(b, "fields").iter().filter_map(text_object).collect();
                let accessory = b
                    .get("accessory")
                    .map(|a| match s(a, "type").unwrap_or("") {
                        "image" => Accessory::Image {
                            url: s(a, "image_url").unwrap_or_default().to_string(),
                            alt: s(a, "alt_text").unwrap_or_default().to_string(),
                        },
                        "button" => Accessory::Button(button(a)),
                        other => Accessory::Other(other.to_string()),
                    });
                Block::Section {
                    text,
                    fields,
                    accessory,
                }
            }
            "context" => Block::Context(
                arr(b, "elements")
                    .iter()
                    .filter_map(|e| match s(e, "type").unwrap_or("") {
                        "image" => Some(Doc(vec![crate::ast::BlockNode::Section(vec![
                            crate::ast::Inline::Text {
                                text: format!("[{}]", s(e, "alt_text").unwrap_or("image")),
                                style: Default::default(),
                            },
                        ])])),
                        _ => text_object(e),
                    })
                    .collect(),
            ),
            "divider" => Block::Divider,
            "image" => Block::Image {
                url: s(b, "image_url").unwrap_or_default().to_string(),
                alt: s(b, "alt_text").unwrap_or_default().to_string(),
                title: b
                    .get("title")
                    .and_then(|t| s(t, "text"))
                    .map(str::to_string),
            },
            "actions" => Block::Actions(
                arr(b, "elements")
                    .iter()
                    .map(|e| match s(e, "type").unwrap_or("") {
                        "button" => button(e),
                        other => Button {
                            label: format!("<{other}>"),
                            url: None,
                            style: None,
                        },
                    })
                    .collect(),
            ),
            "rich_text" => Block::RichText(crate::richtext::parse_block(b)),
            other => Block::Unsupported(other.to_string()),
        });
    }
    out
}

/// Legacy `attachments`, which is still how link unfurls and a great many bots
/// arrive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attachment {
    pub color: Option<String>,
    pub pretext: Option<Doc>,
    pub author: Option<String>,
    pub title: Option<String>,
    pub title_link: Option<String>,
    pub text: Option<Doc>,
    pub fields: Vec<(String, Doc, bool)>,
    pub image_url: Option<String>,
    pub footer: Option<String>,
    pub blocks: Vec<Block>,
}

pub fn parse_attachments(items: &[Value]) -> Vec<Attachment> {
    items
        .iter()
        .map(|a| Attachment {
            color: s(a, "color").map(str::to_string),
            pretext: s(a, "pretext").map(crate::mrkdwn::parse),
            author: s(a, "author_name").map(str::to_string),
            title: s(a, "title").map(str::to_string),
            title_link: s(a, "title_link").map(str::to_string),
            text: s(a, "text").map(crate::mrkdwn::parse),
            fields: arr(a, "fields")
                .iter()
                .map(|f| {
                    (
                        s(f, "title").unwrap_or("").to_string(),
                        crate::mrkdwn::parse(s(f, "value").unwrap_or("")),
                        f.get("short").and_then(Value::as_bool).unwrap_or(false),
                    )
                })
                .collect(),
            image_url: s(a, "image_url").map(str::to_string),
            footer: s(a, "footer").map(str::to_string),
            blocks: parse(arr(a, "blocks")),
        })
        .collect()
}
