//! Parse Slack's `rich_text` block into the same AST the mrkdwn parser
//! produces.
//!
//! This is the shape every message typed in a Slack client arrives in, so it
//! is the common case, not the exotic one. It is walked as untyped JSON rather
//! than deserialised into structs: an unknown element type must be skipped,
//! never fail the whole message, and serde's derive fights that.
//!
//! Unlike `text`, rich_text carries literal strings - they are **not**
//! HTML-escaped, so unescaping here would corrupt a message containing `&amp;`
//! typed literally.

use crate::ast::*;
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

fn style_of(v: &Value) -> Style {
    let st = v.get("style");
    let flag = |k: &str| {
        st.and_then(|s| s.get(k))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    Style {
        bold: flag("bold"),
        italic: flag("italic"),
        strike: flag("strike"),
        code: flag("code"),
    }
}

/// One `rich_text` block's `elements` array into blocks of the AST.
pub fn parse_block(block: &Value) -> Doc {
    let mut out = Vec::new();
    for el in arr(block, "elements") {
        match s(el, "type").unwrap_or("") {
            "rich_text_section" => out.push(BlockNode::Section(inlines(arr(el, "elements")))),
            "rich_text_quote" => out.push(BlockNode::Quote(inlines(arr(el, "elements")))),
            "rich_text_preformatted" => {
                // Its children are text elements whose concatenation is the
                // block; styles inside are meaningless in a code block.
                let mut body = String::new();
                for c in arr(el, "elements") {
                    match s(c, "type").unwrap_or("") {
                        "text" => body.push_str(s(c, "text").unwrap_or("")),
                        "link" => body.push_str(s(c, "url").unwrap_or("")),
                        _ => {}
                    }
                }
                out.push(BlockNode::Preformatted(body));
            }
            "rich_text_list" => {
                let style = match s(el, "style").unwrap_or("bullet") {
                    "ordered" => ListStyle::Ordered,
                    _ => ListStyle::Bullet,
                };
                let indent = el.get("indent").and_then(Value::as_u64).unwrap_or(0).min(8) as u8;
                let items = arr(el, "elements")
                    .iter()
                    .filter(|c| s(c, "type") == Some("rich_text_section"))
                    .map(|c| inlines(arr(c, "elements")))
                    .collect();
                out.push(BlockNode::List {
                    style,
                    indent,
                    items,
                });
            }
            // An element type we do not model. Skipping it loses that piece of
            // the message; failing would lose all of it.
            _ => {}
        }
    }
    if out.is_empty() {
        out.push(BlockNode::Section(Vec::new()));
    }
    Doc(out)
}

pub fn inlines(els: &[Value]) -> Vec<Inline> {
    let mut out = Vec::new();
    for el in els {
        let style = style_of(el);
        match s(el, "type").unwrap_or("") {
            "text" => {
                let text = s(el, "text").unwrap_or("");
                if text.is_empty() {
                    continue;
                }
                // A newline inside a text element is a hard break, and the
                // renderer needs it as structure rather than as a stray \n it
                // would have to notice while wrapping.
                for (i, part) in text.split('\n').enumerate() {
                    if i > 0 {
                        out.push(Inline::Break);
                    }
                    if !part.is_empty() {
                        out.push(Inline::Text {
                            text: part.to_string(),
                            style,
                        });
                    }
                }
            }
            "link" => out.push(Inline::Link {
                url: s(el, "url").unwrap_or_default().to_string(),
                text: s(el, "text").map(str::to_string),
                style,
            }),
            "user" => {
                if let Some(id) = s(el, "user_id") {
                    out.push(Inline::User {
                        id: id.to_string(),
                        label: None,
                    });
                }
            }
            "usergroup" => {
                if let Some(id) = s(el, "usergroup_id") {
                    out.push(Inline::UserGroup {
                        id: id.to_string(),
                        label: None,
                    });
                }
            }
            "channel" => {
                if let Some(id) = s(el, "channel_id") {
                    out.push(Inline::Channel {
                        id: id.to_string(),
                        label: None,
                    });
                }
            }
            "broadcast" => {
                if let Some(b) = s(el, "range").and_then(Broadcast::parse) {
                    out.push(Inline::Broadcast(b));
                }
            }
            "emoji" => {
                if let Some(name) = s(el, "name") {
                    // `unicode` is a hyphen-separated list of code points for
                    // sequences such as flags and ZWJ families.
                    let unicode = s(el, "unicode").and_then(|u| {
                        let mut acc = String::new();
                        for part in u.split('-') {
                            let cp = u32::from_str_radix(part, 16).ok()?;
                            acc.push(char::from_u32(cp)?);
                        }
                        Some(acc)
                    });
                    let skin = el
                        .get("skin_tone")
                        .and_then(Value::as_u64)
                        .filter(|n| (2..=6).contains(n))
                        .map(|n| n as u8);
                    out.push(Inline::Emoji {
                        name: name.to_string(),
                        skin,
                        unicode,
                    });
                }
            }
            "date" => {
                let ts = el.get("timestamp").and_then(Value::as_i64).unwrap_or(0);
                out.push(Inline::Date {
                    ts,
                    format: s(el, "format").unwrap_or("{date_short}").to_string(),
                    url: s(el, "url").map(str::to_string),
                    fallback: s(el, "fallback").unwrap_or("").to_string(),
                });
            }
            // `color` renders as a swatch in Slack; as text here.
            "color" => {
                if let Some(v) = s(el, "value") {
                    out.push(Inline::Text {
                        text: v.to_string(),
                        style,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// A whole message: prefer `blocks`, fall back to `text`.
///
/// The fallback is not a rare path - every bot message, every message from
/// before rich_text existed, and every `message_changed` payload that omits
/// blocks lands here.
pub fn parse_message(msg: &Value) -> Doc {
    for b in arr(msg, "blocks") {
        if s(b, "type") == Some("rich_text") {
            let doc = parse_block(b);
            if !doc.plain().is_empty() {
                return doc;
            }
        }
    }
    crate::mrkdwn::parse(s(msg, "text").unwrap_or(""))
}
