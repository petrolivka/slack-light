//! Slack's message links, taken apart.
//!
//! A permalink is what someone pastes into a chat when they mean "look at
//! this", and being able to follow one without leaving the terminal is most of
//! why the client knows how to jump to a message at all.
//!
//! The shape is `https://<team>.slack.com/archives/<channel>/p<ts>`, where the
//! timestamp has had its dot removed — `1725701900.000100` becomes
//! `p1725701900000100`. A thread reply adds `?thread_ts=…&cid=…`, which is why
//! the query string is parsed rather than ignored.

use crate::{ChannelId, Ts};

/// What a permalink points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub channel: ChannelId,
    pub ts: Ts,
    /// The parent, when the link is to a reply inside a thread.
    pub thread: Option<Ts>,
}

/// Read a Slack message link, or `None` if it is some other URL.
pub fn parse(url: &str) -> Option<Link> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    if !host.ends_with(".slack.com") {
        return None;
    }
    let path = path.strip_prefix("archives/")?;
    let (channel, tail) = path.split_once('/')?;
    if channel.is_empty() {
        return None;
    }

    let (stamp, query) = match tail.split_once('?') {
        Some((s, q)) => (s, Some(q)),
        None => (tail, None),
    };
    let ts = decode(stamp.strip_prefix('p')?)?;

    // `thread_ts` arrives with its dot intact, unlike the one in the path.
    let thread = query
        .and_then(|q| {
            q.split('&')
                .find_map(|kv| kv.strip_prefix("thread_ts="))
                .map(str::to_string)
        })
        .filter(|t| t.contains('.'))
        .map(Ts::new);

    Some(Link {
        channel: ChannelId::new(channel),
        ts,
        thread,
    })
}

/// Put the dot back: the last six digits are the microseconds.
fn decode(digits: &str) -> Option<Ts> {
    if digits.len() < 7 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (secs, micros) = digits.split_at(digits.len() - 6);
    Some(Ts::new(format!("{secs}.{micros}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_message_link() {
        let l = parse("https://slk-dev.slack.com/archives/C0ENG/p1725701900000100").unwrap();
        assert_eq!(l.channel.as_str(), "C0ENG");
        assert_eq!(l.ts.as_str(), "1725701900.000100");
        assert_eq!(l.thread, None);
    }

    #[test]
    fn a_reply_carries_its_parent() {
        let l = parse(
            "https://slk-dev.slack.com/archives/C0ENG/p1725701940000200\
             ?thread_ts=1725701900.000100&cid=C0ENG",
        )
        .unwrap();
        assert_eq!(l.ts.as_str(), "1725701940.000200");
        assert_eq!(l.thread.unwrap().as_str(), "1725701900.000100");
    }

    #[test]
    fn a_direct_message_link_works_the_same() {
        let l = parse("https://slk-dev.slack.com/archives/D0ALICE/p1725701000000100").unwrap();
        assert_eq!(l.channel.as_str(), "D0ALICE");
    }

    #[test]
    fn anything_else_is_not_one() {
        for url in [
            "https://example.com/archives/C0ENG/p1725701900000100",
            "https://slk-dev.slack.com/messages/C0ENG",
            "https://slk-dev.slack.com/archives/C0ENG",
            "https://slk-dev.slack.com/archives/C0ENG/1725701900000100",
            "https://slk-dev.slack.com/archives//p1725701900000100",
            "https://slk-dev.slack.com/archives/C0ENG/pnotanumber",
            "#engineering",
            "",
        ] {
            assert!(parse(url).is_none(), "{url} should not parse");
        }
    }

    #[test]
    fn a_short_stamp_is_refused_rather_than_split_wrongly() {
        // Six digits or fewer would leave nothing before the dot.
        assert!(parse("https://x.slack.com/archives/C1/p000100").is_none());
    }
}
