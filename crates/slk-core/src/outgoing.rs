//! What the user typed, turned into what Slack expects on the wire.
//!
//! The inbound half of this is `mrkdwn`; this is the outbound half, and it
//! is smaller than it looks because Slack accepts almost everything a person
//! types. Only four things have to change:
//!
//! - `&`, `<` and `>` are escaped, because they delimit Slack's own tokens
//! - `@alice` becomes `<@U0ALICE>`, or stays literal if nobody is called that
//! - `#design` becomes `<#C0DESIGN|design>`, likewise
//! - `@here`, `@channel` and `@everyone` become `<!here>` and friends
//!
//! Emoji shortcodes are left exactly as typed: Slack stores `:tada:` and
//! renders it, and rewriting it to the glyph would lose the name the reaction
//! API needs.
//!
//! Nothing is rewritten **inside code**. `@alice` in a snippet is a literal,
//! and a client that mentions somebody because their name appeared in a stack
//! trace is a client people turn off.

use crate::ast::escape;

/// Names to ids, as the interface knows them.
pub trait Lookup {
    fn user_id(&self, name: &str) -> Option<String>;
    fn channel_id(&self, name: &str) -> Option<String>;
    /// A user group's id, for `@design-team`.
    fn usergroup_id(&self, handle: &str) -> Option<String> {
        let _ = handle;
        None
    }
}

/// The degenerate lookup: everything stays literal.
pub struct NoLookup;
impl Lookup for NoLookup {
    fn user_id(&self, _: &str) -> Option<String> {
        None
    }
    fn channel_id(&self, _: &str) -> Option<String> {
        None
    }
}

/// The three names that address a room rather than a person.
const BROADCASTS: [&str; 3] = ["here", "channel", "everyone"];

pub fn encode(text: &str, names: &dyn Lookup) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for (chunk, is_code) in split_code(text) {
        if is_code {
            // Escaped, because Slack's own parser still reads `<` inside a
            // code span — but never rewritten.
            out.push_str(&escape(chunk));
        } else {
            encode_prose(chunk, names, &mut out);
        }
    }
    out
}

fn encode_prose(text: &str, names: &dyn Lookup, out: &mut String) {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut plain = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c != b'@' && c != b'#' {
            i += 1;
            continue;
        }
        // A sigil only counts at a word boundary: an e-mail address is not a
        // mention, and `C#` is not a channel.
        let before_ok = i == 0 || {
            let prev = text[..i].chars().next_back().unwrap();
            !prev.is_alphanumeric() && prev != '_' && prev != '-' && prev != '.' && prev != '/'
        };
        let name: String = text[i + 1..]
            .chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '-' || *ch == '.')
            .collect();
        // Trailing punctuation belongs to the sentence, not to the name.
        let name = name.trim_end_matches('.').to_string();
        if !before_ok || name.is_empty() {
            i += 1;
            continue;
        }
        let token = if c == b'@' {
            if BROADCASTS.contains(&name.as_str()) {
                Some(format!("<!{name}>"))
            } else {
                names
                    .user_id(&name)
                    .map(|id| format!("<@{id}>"))
                    .or_else(|| {
                        names
                            .usergroup_id(&name)
                            .map(|id| format!("<!subteam^{id}>"))
                    })
            }
        } else {
            names.channel_id(&name).map(|id| format!("<#{id}|{name}>"))
        };
        match token {
            Some(t) => {
                out.push_str(&escape(&text[plain..i]));
                out.push_str(&t);
                i += 1 + name.len();
                plain = i;
            }
            // Nobody by that name: leave it as the person wrote it.
            None => i += 1 + name.len(),
        }
    }
    out.push_str(&escape(&text[plain..]));
}

/// Split into runs, saying of each whether it is code. Fenced blocks first,
/// then single-backtick spans inside what is left.
fn split_code(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let (before, tail) = rest.split_at(start);
        push_spans(before, &mut out);
        match tail[3..].find("```") {
            Some(end) => {
                let stop = 3 + end + 3;
                out.push((&tail[..stop], true));
                rest = &tail[stop..];
            }
            // An unclosed fence runs to the end, which is what Slack does
            // with it too.
            None => {
                out.push((tail, true));
                return out;
            }
        }
    }
    push_spans(rest, &mut out);
    out
}

fn push_spans<'a>(text: &'a str, out: &mut Vec<(&'a str, bool)>) {
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        let (before, tail) = rest.split_at(start);
        if !before.is_empty() {
            out.push((before, false));
        }
        match tail[1..].find('`') {
            Some(end) => {
                let stop = 1 + end + 1;
                out.push((&tail[..stop], true));
                rest = &tail[stop..];
            }
            None => {
                out.push((tail, false));
                return;
            }
        }
    }
    if !rest.is_empty() {
        out.push((rest, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir;
    impl Lookup for Dir {
        fn user_id(&self, name: &str) -> Option<String> {
            match name {
                "alice" => Some("U0ALICE".into()),
                "bob" => Some("U0BOB".into()),
                _ => None,
            }
        }
        fn channel_id(&self, name: &str) -> Option<String> {
            (name == "design").then(|| "C0DESIGN".into())
        }
        fn usergroup_id(&self, handle: &str) -> Option<String> {
            (handle == "design-team").then(|| "S0DESIGN".into())
        }
    }

    #[test]
    fn names_become_ids_and_strangers_stay_literal() {
        assert_eq!(encode("hi @alice", &Dir), "hi <@U0ALICE>");
        assert_eq!(encode("hi @nobody", &Dir), "hi @nobody");
        assert_eq!(encode("see #design", &Dir), "see <#C0DESIGN|design>");
        assert_eq!(encode("see #nowhere", &Dir), "see #nowhere");
        assert_eq!(
            encode("@design-team ping", &Dir),
            "<!subteam^S0DESIGN> ping"
        );
    }

    #[test]
    fn the_room_broadcasts_are_their_own_tokens() {
        assert_eq!(encode("@here now", &Dir), "<!here> now");
        assert_eq!(encode("@channel", &Dir), "<!channel>");
        assert_eq!(encode("@everyone", &Dir), "<!everyone>");
    }

    #[test]
    fn a_sigil_only_counts_at_a_word_boundary() {
        // An e-mail address is not a mention of alice.
        assert_eq!(encode("mail me@alice.com", &Dir), "mail me@alice.com");
        assert_eq!(encode("(@alice)", &Dir), "(<@U0ALICE>)");
        assert_eq!(encode("@alice, hello", &Dir), "<@U0ALICE>, hello");
        // Trailing punctuation belongs to the sentence.
        assert_eq!(encode("ask @alice.", &Dir), "ask <@U0ALICE>.");
    }

    #[test]
    fn code_is_never_rewritten() {
        assert_eq!(encode("`@alice`", &Dir), "`@alice`");
        assert_eq!(
            encode("```\ncc @alice\n```", &Dir),
            "```\ncc @alice\n```",
            "a fenced block is literal to the end of the fence"
        );
        assert_eq!(
            encode("say @alice then `@bob` then @bob", &Dir),
            "say <@U0ALICE> then `@bob` then <@U0BOB>",
        );
        // An unclosed fence swallows the rest, the way Slack renders it.
        assert_eq!(encode("```\n@alice", &Dir), "```\n@alice");
    }

    #[test]
    fn the_three_special_characters_are_escaped_everywhere() {
        assert_eq!(
            encode("a < b && c > d", &Dir),
            "a &lt; b &amp;&amp; c &gt; d"
        );
        assert_eq!(encode("`a < b`", &Dir), "`a &lt; b`");
        // And the tokens we add are not escaped afterwards.
        assert_eq!(encode("@alice < 3", &Dir), "<@U0ALICE> &lt; 3");
    }

    #[test]
    fn shortcodes_are_left_alone() {
        assert_eq!(encode("ship it :rocket:", &Dir), "ship it :rocket:");
    }

    #[test]
    fn nothing_is_rewritten_without_a_directory() {
        assert_eq!(encode("@alice #design", &NoLookup), "@alice #design");
    }
}
