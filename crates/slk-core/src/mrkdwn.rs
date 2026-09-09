//! Parse Slack's mrkdwn into the AST.
//!
//! mrkdwn is not Markdown. `*bold*` not `**bold**`, `_italic_`, `~strike~`, no
//! link syntax at all - links arrive already tokenised as `<url|text>` - and
//! `&`, `<`, `>` are HTML-escaped in the source text.
//!
//! Order matters and is the one thing that is easy to get wrong: `<…>` tokens
//! are recognised **before** entities are unescaped. Doing it the other way
//! round turns a literal `&lt;@U1&gt;` that a user typed into a real mention.

use crate::ast::*;

pub fn parse(src: &str) -> Doc {
    let mut nodes = Vec::new();
    let lines: Vec<&str> = src.split('\n').collect();
    let mut i = 0;

    while i < lines.len() {
        // Each pass must consume a line. A block detector that recognises a
        // line but whose branch then declines to take it would otherwise spin
        // here forever - which is exactly what happened when the quote branch
        // matched `>` and the section scanner matched `&gt;` as well.
        let progress_from = i;
        let line = lines[i];
        let trimmed = line.trim_start();

        // Fenced code block. Slack allows ```code``` on one line too.
        if let Some(rest) = trimmed.strip_prefix("```") {
            if let Some(inner) = rest.strip_suffix("```") {
                if !rest.ends_with("```") || rest.len() < 3 {
                    // guard against "```" alone matching its own suffix
                } else {
                    nodes.push(BlockNode::Preformatted(inner.to_string()));
                    i += 1;
                    continue;
                }
            }
            let mut body = Vec::new();
            i += 1;
            let mut closed = false;
            while i < lines.len() {
                if lines[i].trim_end() == "```" {
                    closed = true;
                    i += 1;
                    break;
                }
                body.push(lines[i]);
                i += 1;
            }
            let _ = closed; // an unterminated fence still renders as a block
            nodes.push(BlockNode::Preformatted(body.join("\n")));
            continue;
        }

        // Block quote: consecutive quote lines are one quote. Slack escapes the
        // marker along with everything else in the `text` fallback, so a real
        // message says `&gt; ` and only something we generated ourselves says
        // `> `. Matching one form and not the other is how the section scanner
        // below and this branch came to disagree about where a block starts.
        if quote_body(trimmed).is_some() {
            let mut body = Vec::new();
            while i < lines.len() {
                let Some(rest) = quote_body(lines[i].trim_start()) else {
                    break;
                };
                body.push(rest);
                i += 1;
            }
            nodes.push(BlockNode::Quote(inline(&body.join("\n"))));
            continue;
        }

        // Bullet or ordered list. mrkdwn has no list syntax, but the `text`
        // fallback of a rich_text list is written this way, and bots write it
        // by hand, so reading it back as a list is what users expect to see.
        if let Some((style, indent)) = list_marker(line) {
            let mut items = Vec::new();
            while i < lines.len() {
                let Some((s, ind)) = list_marker(lines[i]) else {
                    break;
                };
                if s != style || ind != indent {
                    break;
                }
                items.push(inline(strip_list_marker(lines[i])));
                i += 1;
            }
            nodes.push(BlockNode::List {
                style,
                indent,
                items,
            });
            continue;
        }

        // A run of ordinary lines is one section with hard breaks inside it,
        // which is how Slack renders a multi-line message: one paragraph, not
        // several blocks.
        let mut body = Vec::new();
        while i < lines.len() {
            let t = lines[i].trim_start();
            if t.starts_with("```") || quote_body(t).is_some() || list_marker(lines[i]).is_some() {
                break;
            }
            body.push(lines[i]);
            i += 1;
        }
        nodes.push(BlockNode::Section(inline(&body.join("\n"))));

        debug_assert!(
            i > progress_from,
            "block scanner made no progress at line {progress_from}"
        );
        if i == progress_from {
            // Release builds take the line rather than spinning: a message that
            // renders oddly is recoverable, a frozen UI is not.
            i += 1;
        }
    }

    if nodes.is_empty() {
        nodes.push(BlockNode::Section(Vec::new()));
    }
    Doc(nodes)
}

/// The text after a block-quote marker, in either the escaped or the raw form.
fn quote_body(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix("&gt;")
        .or_else(|| line.strip_prefix('>'))?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

fn list_marker(line: &str) -> Option<(ListStyle, u8)> {
    let indent = (line.len() - line.trim_start().len()) / 4;
    let t = line.trim_start();
    if t.starts_with("• ") || t.starts_with("- ") || t.starts_with("* ") && t.len() > 2 {
        // `* ` would also be an unterminated bold; a bullet needs the space and
        // no closing marker on the line.
        if t.starts_with("* ") && t[2..].contains('*') {
            return None;
        }
        return Some((ListStyle::Bullet, indent.min(8) as u8));
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() && t[digits.len()..].starts_with(". ") {
        return Some((ListStyle::Ordered, indent.min(8) as u8));
    }
    None
}

fn strip_list_marker(line: &str) -> &str {
    let t = line.trim_start();
    for p in ["• ", "- ", "* "] {
        if let Some(r) = t.strip_prefix(p) {
            return r;
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    t[digits..].strip_prefix(". ").unwrap_or(t)
}

/// Parse the inline level. `style` is inherited from an enclosing `*…*` etc.
pub fn inline(src: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    inline_into(src, Style::default(), &mut out);
    merge_text(&mut out);
    out
}

fn inline_into(src: &str, style: Style, out: &mut Vec<Inline>) {
    let b = src.as_bytes();
    let mut i = 0;
    let mut text = String::new();

    macro_rules! flush {
        () => {
            if !text.is_empty() {
                out.push(Inline::Text {
                    text: unescape(&text),
                    style,
                });
                text.clear();
            }
        };
    }

    while i < b.len() {
        let c = b[i];

        // `<…>` tokens, before any unescaping.
        if c == b'<' {
            if let Some(end) = find_byte(b, i + 1, b'>') {
                let raw = &src[i + 1..end];
                if let Some(node) = token(raw, style) {
                    flush!();
                    out.push(node);
                    i = end + 1;
                    continue;
                }
            }
            text.push('<');
            i += 1;
            continue;
        }

        // Code spans swallow everything, including formatting characters.
        if c == b'`' && !style.code {
            if let Some(end) = find_byte(b, i + 1, b'`') {
                if end > i + 1 {
                    flush!();
                    let mut s = style;
                    s.code = true;
                    out.push(Inline::Text {
                        text: unescape(&src[i + 1..end]),
                        style: s,
                    });
                    i = end + 1;
                    continue;
                }
            }
            text.push('`');
            i += 1;
            continue;
        }

        // Emphasis. Not applied inside code.
        if !style.code && matches!(c, b'*' | b'_' | b'~') {
            let already = match c {
                b'*' => style.bold,
                b'_' => style.italic,
                _ => style.strike,
            };
            if !already && opens(b, i) {
                if let Some(end) = closing(b, i + 1, c) {
                    flush!();
                    let mut s = style;
                    match c {
                        b'*' => s.bold = true,
                        b'_' => s.italic = true,
                        _ => s.strike = true,
                    }
                    inline_into(&src[i + 1..end], s, out);
                    i = end + 1;
                    continue;
                }
            }
            text.push(c as char);
            i += 1;
            continue;
        }

        // `:shortcode:`, optionally followed by `::skin-tone-N:`.
        if c == b':' {
            if let Some((name, skin, next)) = emoji(src, i) {
                flush!();
                out.push(Inline::Emoji {
                    name,
                    skin,
                    unicode: None,
                });
                i = next;
                continue;
            }
            text.push(':');
            i += 1;
            continue;
        }

        if c == b'\n' {
            flush!();
            out.push(Inline::Break);
            i += 1;
            continue;
        }

        // Copy one whole UTF-8 character.
        let ch = src[i..].chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }
    flush!();
}

fn find_byte(b: &[u8], from: usize, needle: u8) -> Option<usize> {
    (from..b.len()).find(|&i| b[i] == needle)
}

/// A delimiter opens if it sits at a word boundary on its left and has
/// something other than whitespace on its right.
fn opens(b: &[u8], i: usize) -> bool {
    let left_ok = i == 0 || {
        let p = b[i - 1];
        p.is_ascii_whitespace() || matches!(p, b'(' | b'[' | b'{' | b'"' | b'\'' | b'<' | b'|')
    };
    let right_ok = i + 1 < b.len() && !b[i + 1].is_ascii_whitespace();
    left_ok && right_ok
}

/// Find the matching close for the delimiter opened at `from - 1`: the first
/// one preceded by a non-space and followed by end, whitespace or punctuation.
fn closing(b: &[u8], from: usize, marker: u8) -> Option<usize> {
    let mut i = from;
    while i < b.len() {
        if b[i] == b'`' {
            // Skip a code span whole, so `*a `b*c` d*` closes at the last `*`.
            if let Some(e) = find_byte(b, i + 1, b'`') {
                i = e + 1;
                continue;
            }
        }
        if b[i] == marker && i > from && !b[i - 1].is_ascii_whitespace() {
            let right_ok = i + 1 >= b.len() || {
                let n = b[i + 1];
                n.is_ascii_whitespace()
                    || matches!(
                        n,
                        b'.' | b','
                            | b'!'
                            | b'?'
                            | b';'
                            | b':'
                            | b')'
                            | b']'
                            | b'}'
                            | b'"'
                            | b'\''
                            | b'*'
                            | b'_'
                            | b'~'
                    )
            };
            if right_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn emoji(src: &str, i: usize) -> Option<(String, Option<u8>, usize)> {
    let rest = &src[i + 1..];
    let end = rest.find(':')?;
    let name = &rest[..end];
    if name.is_empty()
        || name.len() > 100
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '\'' | '.'))
    {
        return None;
    }
    let mut next = i + 1 + end + 1;
    let mut skin = None;
    // `:wave::skin-tone-3:` - the modifier is a second shortcode.
    if let Some(tail) = src.get(next..) {
        if let Some(t) = tail.strip_prefix(":skin-tone-") {
            if let Some(d) = t.chars().next().and_then(|c| c.to_digit(10)) {
                if t[1..].starts_with(':') && (2..=6).contains(&d) {
                    skin = Some(d as u8);
                    next += ":skin-tone-".len() + 2;
                }
            }
        }
    }
    Some((name.to_string(), skin, next))
}

fn token(raw: &str, style: Style) -> Option<Inline> {
    if raw.is_empty() {
        return None;
    }
    let (head, label) = match raw.find('|') {
        Some(p) => (&raw[..p], Some(unescape(&raw[p + 1..]))),
        None => (raw, None),
    };

    if let Some(id) = head.strip_prefix('@') {
        return valid_id(id).then(|| Inline::User {
            id: id.to_string(),
            label,
        });
    }
    if let Some(id) = head.strip_prefix('#') {
        return valid_id(id).then(|| Inline::Channel {
            id: id.to_string(),
            label,
        });
    }
    if let Some(bang) = head.strip_prefix('!') {
        if let Some(id) = bang.strip_prefix("subteam^") {
            return valid_id(id).then(|| Inline::UserGroup {
                id: id.to_string(),
                label,
            });
        }
        if let Some(rest) = bang.strip_prefix("date^") {
            let mut parts = rest.split('^');
            let ts: i64 = parts.next()?.parse().ok()?;
            let format = parts.next()?.to_string();
            let url = parts.next().map(str::to_string);
            return Some(Inline::Date {
                ts,
                format,
                url,
                fallback: label.unwrap_or_default(),
            });
        }
        if let Some(b) = Broadcast::parse(bang) {
            return Some(Inline::Broadcast(b));
        }
        // An unknown `<!something>` is a Slack feature we do not model. Showing
        // its label is strictly better than showing the raw token.
        return label.map(|l| Inline::Text { text: l, style });
    }

    // Anything else with a scheme is a link. The URL is escaped like the rest of
    // the text - `?a=1&amp;b=2` - and handing that to a browser makes a
    // different request than the one the sender wrote.
    if head.contains("://") || head.starts_with("mailto:") || head.starts_with("tel:") {
        return Some(Inline::Link {
            url: unescape(head),
            text: label,
            style,
        });
    }
    None
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Adjacent text runs with the same style are one run. Without this, parsing
/// what `emit` wrote produces a different tree for the same message and the
/// round-trip property fails for reasons that are not real differences.
fn merge_text(xs: &mut Vec<Inline>) {
    let mut i = 0;
    while i + 1 < xs.len() {
        let merge = matches!(
            (&xs[i], &xs[i + 1]),
            (Inline::Text { style: a, .. }, Inline::Text { style: b, .. }) if a == b
        );
        if merge {
            let Inline::Text { text: next, .. } = xs.remove(i + 1) else {
                unreachable!()
            };
            let Inline::Text { text, .. } = &mut xs[i] else {
                unreachable!()
            };
            text.push_str(&next);
        } else {
            i += 1;
        }
    }
    xs.retain(|x| !matches!(x, Inline::Text { text, .. } if text.is_empty()));
}
