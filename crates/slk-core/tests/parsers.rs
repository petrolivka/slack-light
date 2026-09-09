//! What the parsers must do, and must never do.
//!
//! Two kinds of test. The first pins behaviour that is easy to get subtly
//! wrong - escaping order, delimiter rules, the emoji table. The second proves
//! the parsers degrade rather than panic, which is the property that keeps a
//! Slack shape change from taking the whole client down mid-conversation.

use serde_json::json;
use slk_core::ast::{self, BlockNode, Inline, ListStyle, Names, Style};
use slk_core::{mrkdwn, richtext};

struct Dir;
impl Names for Dir {
    fn user(&self, id: &str) -> Option<&str> {
        (id == "U1").then_some("alice")
    }
    fn channel(&self, id: &str) -> Option<&str> {
        (id == "C1").then_some("design")
    }
}

#[test]
fn entities_are_unescaped_exactly_once_and_after_tokenising() {
    // The trap: unescape first and `&lt;@U1&gt;` becomes a real mention.
    let d = mrkdwn::parse("5 &lt; 6 &amp;&amp; a &lt;@U1&gt; b <@U1>");
    let plain = d.plain_with(&Dir);
    assert!(plain.contains("5 < 6 && a <@U1> b @alice"), "got {plain:?}");
}

#[test]
fn emphasis_needs_word_boundaries() {
    // A star inside a word is a star, not a delimiter - otherwise every
    // `a*b*c` in a code-ish message turns bold.
    let d = mrkdwn::parse("2*3*4 and *real* and snake_case_word and _yes_");
    let plain = d.plain();
    assert_eq!(plain, "2*3*4 and real and snake_case_word and yes");
    let BlockNode::Section(xs) = &d.0[0] else {
        panic!()
    };
    let bold: Vec<_> = xs
        .iter()
        .filter(|x| matches!(x, Inline::Text { style, .. } if style.bold))
        .collect();
    assert_eq!(bold.len(), 1, "exactly one bold run: {xs:#?}");
}

#[test]
fn code_spans_swallow_formatting() {
    let d = mrkdwn::parse("run `a *b* c` then *bold*");
    let BlockNode::Section(xs) = &d.0[0] else {
        panic!()
    };
    let code = xs
        .iter()
        .find_map(|x| match x {
            Inline::Text { text, style } if style.code => Some(text.clone()),
            _ => None,
        })
        .expect("a code run");
    assert_eq!(code, "a *b* c", "formatting inside code stays literal");
}

#[test]
fn round_trip_holds_for_every_construct() {
    for src in [
        "*bold* _italic_ ~struck~ `code`",
        "cc <@U1> in <#C1|design> see <https://e.com/a|link> <!here>",
        "line one\nline two",
        "> quoted\n> more",
        "```\nfn main() {}\n```",
        "• one\n• two",
        "1. first\n2. second",
        ":tada: :+1::skin-tone-3:",
        "<!date^1725840000^{date_short}|Sep 9>",
        "5 &lt; 6 &amp;&amp; 7 &gt; 6",
        "*_nested both_*",
    ] {
        let once = mrkdwn::parse(src);
        let again = mrkdwn::parse(&ast::emit(&once));
        assert_eq!(
            once,
            again,
            "round trip failed for {src:?}\nemitted: {:?}",
            ast::emit(&once)
        );
    }
}

#[test]
fn slack_shortcodes_resolve_and_unknown_ones_survive() {
    assert_eq!(
        slk_core::emoji::shortcode("+1", None).as_deref(),
        Some("👍")
    );
    assert_eq!(
        slk_core::emoji::shortcode("tada", None).as_deref(),
        Some("🎉")
    );
    // A workspace's custom emoji is not in any Unicode table and must not
    // become an error or an empty string.
    assert_eq!(slk_core::emoji::shortcode("custom_partyparrot", None), None);
}

#[test]
fn rich_text_beats_the_text_fallback() {
    let msg = json!({
        "type": "message", "text": "fallback that must not be used",
        "blocks": [{ "type": "rich_text", "elements": [
            { "type": "rich_text_section", "elements": [{ "type": "text", "text": "the real body" }] }
        ]}]
    });
    assert_eq!(richtext::parse_message(&msg).plain(), "the real body");
}

#[test]
fn text_fallback_is_used_when_blocks_carry_nothing_renderable() {
    let msg = json!({
        "type": "message", "text": "*the fallback*",
        "blocks": [{ "type": "rich_text", "elements": [
            { "type": "rich_text_section", "elements": [{ "type": "unknown_2027_element" }] }
        ]}]
    });
    assert_eq!(richtext::parse_message(&msg).plain(), "the fallback");
}

#[test]
fn rich_text_lists_keep_style_and_indent() {
    let block = json!({ "type": "rich_text", "elements": [
        { "type": "rich_text_list", "style": "ordered", "indent": 2, "elements": [
            { "type": "rich_text_section", "elements": [{ "type": "text", "text": "a" }] },
            { "type": "rich_text_section", "elements": [{ "type": "text", "text": "b" }] }
        ]}
    ]});
    let d = richtext::parse_block(&block);
    let BlockNode::List {
        style,
        indent,
        items,
    } = &d.0[0]
    else {
        panic!("{:?}", d.0[0])
    };
    assert_eq!(*style, ListStyle::Ordered);
    assert_eq!(*indent, 2);
    assert_eq!(items.len(), 2);
}

/// Parsers must degrade, never panic. Every mutation here has a real-world
/// analogue in a Slack shape change.
#[test]
fn degenerate_json_never_panics() {
    let cases = vec![
        json!(null),
        json!(7),
        json!("a bare string"),
        json!({}),
        json!({ "blocks": null }),
        json!({ "blocks": "not an array" }),
        json!({ "blocks": [null, 1, "x", {}, { "type": null }] }),
        json!({ "blocks": [{ "type": "rich_text" }] }),
        json!({ "blocks": [{ "type": "rich_text", "elements": {} }] }),
        json!({ "blocks": [{ "type": "rich_text", "elements": [{ "type": "rich_text_section" }] }] }),
        json!({ "blocks": [{ "type": "rich_text", "elements": [
            { "type": "rich_text_section", "elements": [{ "type": "user" }, { "type": "emoji" }, { "type": "date" }] }]}]}),
        json!({ "text": 12345 }),
        json!({ "text": "" }),
    ];
    for c in cases {
        let doc = richtext::parse_message(&c);
        let _ = doc.plain_with(&Dir);
        let _ = ast::emit(&doc);
    }
}

#[test]
fn malformed_mrkdwn_never_panics_and_never_loses_the_text() {
    for src in [
        "<@",
        "<",
        ">",
        "<@U1",
        "<!date^notanumber^{x}|f>",
        "```unterminated",
        "`unterminated",
        "*unterminated",
        "<!subteam^>",
        "<https://",
        ":",
        "::",
        ":a",
        &":x:".repeat(500),
        &"*".repeat(500),
        "\u{0}\u{1}\u{7f}",
    ] {
        let d = mrkdwn::parse(src);
        let _ = ast::emit(&d);
        let _ = d.plain();
    }
}

#[test]
fn styles_that_have_no_marker_still_survive_a_round_trip() {
    // Bold+italic+strike together has no single mrkdwn marker; the emitter
    // nests them, and the parser has to unpick that nesting.
    let doc = ast::Doc(vec![BlockNode::Section(vec![Inline::Text {
        text: "all three".into(),
        style: Style {
            bold: true,
            italic: true,
            strike: true,
            code: false,
        },
    }])]);
    assert_eq!(mrkdwn::parse(&ast::emit(&doc)), doc);
}

/// Both of these came out of a real capture, not out of the documentation.
/// Slack escapes `&`, `<` and `>` throughout the `text` fallback, including
/// inside a link token and in front of a block quote, and a parser written
/// from the docs alone misses both.
#[test]
fn urls_in_tokens_are_unescaped() {
    let src = "logs: <https://example.com/d?orgId=1&amp;from=now-6h&amp;to=now|dashboard>";
    let d = mrkdwn::parse(src);
    let BlockNode::Section(xs) = &d.0[0] else {
        panic!()
    };
    let url = xs
        .iter()
        .find_map(|x| match x {
            Inline::Link { url, .. } => Some(url.clone()),
            _ => None,
        })
        .expect("a link");
    assert_eq!(url, "https://example.com/d?orgId=1&from=now-6h&to=now");
    // And it must survive being sent again.
    assert_eq!(mrkdwn::parse(&ast::emit(&d)), d);
}

#[test]
fn block_quotes_are_recognised_in_their_escaped_form() {
    for src in ["&gt; one\n&gt; two", "> one\n> two"] {
        let d = mrkdwn::parse(src);
        assert!(
            matches!(d.0[0], BlockNode::Quote(_)),
            "{src:?} did not parse as a quote: {:?}",
            d.0[0]
        );
        assert_eq!(d.plain(), "one\ntwo");
    }
}

/// From the first run against a live workspace: `client.userBoot` returns
/// direct messages in an `ims` array without repeating `is_im` on each object,
/// so every DM was stored as a public channel with an empty name and the
/// sidebar showed a column of bare `#` glyphs.
#[test]
fn conversation_kind_falls_back_to_the_id_prefix() {
    use slk_core::{Conversation, ConversationKind, TeamId};
    let team = TeamId::new("T1");

    let dm = Conversation::parse(&team, &serde_json::json!({"id": "D0ABC", "user": "U9"}))
        .expect("a DM");
    assert!(
        matches!(&dm.kind, ConversationKind::Dm { peer } if peer.as_str() == "U9"),
        "got {:?}",
        dm.kind
    );
    assert!(dm.is_member, "a DM is always ours to read");

    let group = Conversation::parse(
        &team,
        &serde_json::json!({"id": "G0ABC", "members": ["U1", "U2"]}),
    )
    .expect("a group");
    assert!(
        matches!(group.kind, ConversationKind::Mpim { .. }),
        "got {:?}",
        group.kind
    );

    let public = Conversation::parse(&team, &serde_json::json!({"id": "C0ABC", "name": "eng"}))
        .expect("a channel");
    assert_eq!(public.kind, ConversationKind::Public);
    assert_eq!(public.name, "eng");

    // An explicit flag still wins over the prefix.
    let private = Conversation::parse(
        &team,
        &serde_json::json!({"id": "C0XYZ", "name": "leads", "is_private": true}),
    )
    .expect("a private channel");
    assert_eq!(private.kind, ConversationKind::Private);
}

/// Whether a message mentions you decides whether you are interrupted, so it
/// has to be right in both directions.
#[test]
fn mentions_are_detected_only_when_they_are_mentions() {
    use slk_core::{ChannelId, Message, TeamId, UserId};
    let me = UserId::new("U1");
    let team = TeamId::new("T1");
    let ch = ChannelId::new("C1");
    let parse = |text: &str| {
        Message::parse(
            &team,
            &ch,
            &me,
            &serde_json::json!({"ts": "1.0", "user": "U9", "text": text}),
        )
        .expect("a message")
    };

    for text in [
        "cc <@U1> please",
        "<@U1>",
        "everyone see <!here>",
        "<!channel> heads up",
    ] {
        assert!(parse(text).mentions(&me), "{text:?} should mention U1");
    }

    for text in [
        "nothing to see",
        "cc <@U2> please",
        "the string U1 appears here",
        "`<@U1>` inside code is a quotation",
    ] {
        assert!(!parse(text).mentions(&me), "{text:?} should not mention U1");
    }
}

/// The store holds what Slack sent, not a summary of it.
///
/// An earlier version wrote a handful of fields and rebuilt messages from
/// those, so a restart showed every message stripped of its files, reactions
/// and Block Kit — intact on screen until you closed the client, and
/// impoverished when you opened it again.
#[test]
fn a_message_round_trips_through_its_raw_form() {
    use slk_core::{ChannelId, Message, TeamId, UserId};
    let team = TeamId::new("T1");
    let ch = ChannelId::new("C1");
    let me = UserId::new("U0");

    let original = serde_json::json!({
        "type": "message", "user": "U1", "ts": "1.000100",
        "text": "here it is",
        "files": [{"id": "F1", "name": "shot.png", "mimetype": "image/png",
                   "size": 1234, "url_private": "https://files.slack.com/F1"}],
        "reactions": [{"name": "tada", "count": 2, "users": ["U0"]}],
        "blocks": [{"type": "section", "text": {"type": "mrkdwn", "text": "*hi*"}}],
        "reply_count": 3,
    });

    let first = Message::parse(&team, &ch, &me, &original).expect("parses");
    assert_eq!(first.files.len(), 1);
    assert_eq!(first.reactions.len(), 1);
    assert_eq!(first.blocks.len(), 1);

    // What the store would write, read back.
    let reloaded: serde_json::Value = serde_json::from_str(&first.raw).expect("raw is JSON");
    let second = Message::parse(&team, &ch, &me, &reloaded).expect("parses again");

    assert_eq!(second.files, first.files, "files survive the store");
    assert_eq!(second.reactions, first.reactions, "reactions survive");
    assert_eq!(second.blocks, first.blocks, "Block Kit survives");
    assert_eq!(second.reply_count, 3);
    assert!(second.files[0].url_private.is_some(), "and so does the URL");
}
