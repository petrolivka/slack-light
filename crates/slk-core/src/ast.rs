//! The rich-text AST both parsers produce and the renderer consumes.
//!
//! Slack sends a user's message twice: as `blocks[0].type == "rich_text"`,
//! which is what the composer emits, and as a `text` field in mrkdwn, which is
//! the fallback every bot and every old message still uses. Rendering from two
//! different shapes would mean two renderers and two sets of bugs, so both are
//! parsed into this one tree.

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

impl Style {
    pub fn is_plain(self) -> bool {
        self == Style::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Broadcast {
    Here,
    Channel,
    Everyone,
}

impl Broadcast {
    pub fn as_str(self) -> &'static str {
        match self {
            Broadcast::Here => "here",
            Broadcast::Channel => "channel",
            Broadcast::Everyone => "everyone",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "here" => Some(Broadcast::Here),
            "channel" => Some(Broadcast::Channel),
            "everyone" => Some(Broadcast::Everyone),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text {
        text: String,
        style: Style,
    },
    Link {
        url: String,
        /// `None` when the message wrote `<url>` with no label.
        text: Option<String>,
        style: Style,
    },
    User {
        id: String,
        /// Slack sometimes sends `<@U123|name>`; the label is a stale cache and
        /// the id is authoritative, but keeping it lets an unknown user still
        /// render as a name rather than a raw id.
        label: Option<String>,
    },
    UserGroup {
        id: String,
        label: Option<String>,
    },
    Channel {
        id: String,
        label: Option<String>,
    },
    Broadcast(Broadcast),
    Emoji {
        name: String,
        skin: Option<u8>,
        /// Filled in by the renderer from the emoji table, not by the parser.
        unicode: Option<String>,
    },
    Date {
        ts: i64,
        format: String,
        url: Option<String>,
        fallback: String,
    },
    /// A hard line break inside one section.
    Break,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStyle {
    Bullet,
    Ordered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockNode {
    Section(Vec<Inline>),
    List {
        style: ListStyle,
        indent: u8,
        items: Vec<Vec<Inline>>,
    },
    /// A fenced code block. Whitespace is preserved exactly; the renderer never
    /// wraps it.
    Preformatted(String),
    Quote(Vec<Inline>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Doc(pub Vec<BlockNode>);

/// Name lookup for the plain-text projection.
///
/// Without it a desktop notification reads "@U01ALICE said…" and the search
/// index stores ids rather than the names people actually search for. The
/// trait keeps the AST free of any dependency on the UI's context type.
pub trait Names {
    fn user(&self, _id: &str) -> Option<&str> {
        None
    }
    fn channel(&self, _id: &str) -> Option<&str> {
        None
    }
    fn usergroup(&self, _id: &str) -> Option<&str> {
        None
    }
}

/// The degenerate lookup, for callers that have no directory yet.
pub struct NoNames;
impl Names for NoNames {}

impl Doc {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The plain-text projection with ids left as ids. Prefer `plain_with`
    /// wherever a directory is available.
    pub fn plain(&self) -> String {
        self.plain_with(&NoNames)
    }

    /// The plain-text projection, used for the local search index, for
    /// notifications, and for `y` (copy message text).
    pub fn plain_with(&self, names: &dyn Names) -> String {
        let mut out = String::new();
        for (i, node) in self.0.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            match node {
                BlockNode::Section(xs) | BlockNode::Quote(xs) => plain_inlines(xs, names, &mut out),
                BlockNode::Preformatted(s) => out.push_str(s),
                BlockNode::List { items, .. } => {
                    for (j, item) in items.iter().enumerate() {
                        if j > 0 {
                            out.push('\n');
                        }
                        plain_inlines(item, names, &mut out);
                    }
                }
            }
        }
        out
    }
}

fn plain_inlines(xs: &[Inline], names: &dyn Names, out: &mut String) {
    for x in xs {
        match x {
            Inline::Text { text, .. } => out.push_str(text),
            Inline::Link { url, text, .. } => out.push_str(text.as_deref().unwrap_or(url)),
            Inline::User { id, label } => {
                out.push('@');
                out.push_str(names.user(id).or(label.as_deref()).unwrap_or(id));
            }
            Inline::UserGroup { id, label } => {
                out.push('@');
                let n = names.usergroup(id).or(label.as_deref()).unwrap_or(id);
                out.push_str(n.trim_start_matches('@'));
            }
            Inline::Channel { id, label } => {
                out.push('#');
                out.push_str(names.channel(id).or(label.as_deref()).unwrap_or(id));
            }
            Inline::Broadcast(b) => {
                out.push('@');
                out.push_str(b.as_str());
            }
            // The unicode field is only present on rich_text; a shortcode from
            // mrkdwn has to be resolved here or the projection keeps ":tada:".
            Inline::Emoji {
                name,
                skin,
                unicode,
            } => match unicode
                .clone()
                .or_else(|| crate::emoji::shortcode(name, *skin))
            {
                Some(u) => out.push_str(&u),
                None => {
                    let _ = write!(out, ":{name}:");
                }
            },
            Inline::Date { fallback, .. } => out.push_str(fallback),
            Inline::Break => out.push('\n'),
        }
    }
}

/// Escape the three characters Slack treats as special in message text. Applied
/// to what the *user typed* before our own `<@U…>` tokens are added, never to a
/// whole assembled message.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Undo Slack's escaping. Applied to text *after* `<…>` tokens have been split
/// out, never before - otherwise an escaped `&lt;@U1&gt;` would be re-read as a
/// real mention.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Render the AST back to mrkdwn. This is the outbound path (what we send when
/// the user presses Enter) and, in tests, the other half of the round-trip
/// property that keeps the parser honest.
pub fn emit(doc: &Doc) -> String {
    let mut out = String::new();
    for (i, node) in doc.0.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match node {
            BlockNode::Section(xs) => emit_inlines(xs, &mut out),
            BlockNode::Quote(xs) => {
                let mut inner = String::new();
                emit_inlines(xs, &mut inner);
                for (j, line) in inner.split('\n').enumerate() {
                    if j > 0 {
                        out.push('\n');
                    }
                    out.push_str("> ");
                    out.push_str(line);
                }
            }
            BlockNode::Preformatted(s) => {
                out.push_str("```\n");
                out.push_str(s);
                if !s.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```");
            }
            BlockNode::List {
                style,
                indent,
                items,
            } => {
                for (j, item) in items.iter().enumerate() {
                    if j > 0 {
                        out.push('\n');
                    }
                    for _ in 0..*indent {
                        out.push_str("    ");
                    }
                    match style {
                        ListStyle::Bullet => out.push_str("• "),
                        ListStyle::Ordered => {
                            let _ = write!(out, "{}. ", j + 1);
                        }
                    }
                    emit_inlines(item, &mut out);
                }
            }
        }
    }
    out
}

fn emit_inlines(xs: &[Inline], out: &mut String) {
    for x in xs {
        match x {
            Inline::Text { text, style } => {
                let marks = marks_for(*style);
                out.push_str(marks.0);
                out.push_str(&escape(text));
                out.push_str(marks.1);
            }
            Inline::Link { url, text, style } => {
                let marks = marks_for(*style);
                out.push_str(marks.0);
                match text {
                    Some(t) => {
                        let _ = write!(out, "<{}|{}>", escape(url), escape(t));
                    }
                    None => {
                        let _ = write!(out, "<{}>", escape(url));
                    }
                }
                out.push_str(marks.1);
            }
            Inline::User { id, label } => match label {
                Some(l) => {
                    let _ = write!(out, "<@{id}|{l}>");
                }
                None => {
                    let _ = write!(out, "<@{id}>");
                }
            },
            Inline::UserGroup { id, label } => match label {
                Some(l) => {
                    let _ = write!(out, "<!subteam^{id}|{l}>");
                }
                None => {
                    let _ = write!(out, "<!subteam^{id}>");
                }
            },
            Inline::Channel { id, label } => match label {
                Some(l) => {
                    let _ = write!(out, "<#{id}|{l}>");
                }
                None => {
                    let _ = write!(out, "<#{id}>");
                }
            },
            Inline::Broadcast(b) => {
                let _ = write!(out, "<!{}>", b.as_str());
            }
            Inline::Emoji { name, skin, .. } => match skin {
                Some(n) => {
                    let _ = write!(out, ":{name}::skin-tone-{n}:");
                }
                None => {
                    let _ = write!(out, ":{name}:");
                }
            },
            Inline::Date {
                ts,
                format,
                url,
                fallback,
            } => match url {
                Some(u) => {
                    let _ = write!(out, "<!date^{ts}^{format}^{u}|{fallback}>");
                }
                None => {
                    let _ = write!(out, "<!date^{ts}^{format}|{fallback}>");
                }
            },
            Inline::Break => out.push('\n'),
        }
    }
}

/// Code wins over the other three: Slack does not apply bold inside a code
/// span, and emitting `*` inside backticks would render the asterisk literally.
fn marks_for(s: Style) -> (&'static str, &'static str) {
    if s.code {
        return ("`", "`");
    }
    match (s.bold, s.italic, s.strike) {
        (false, false, false) => ("", ""),
        (true, false, false) => ("*", "*"),
        (false, true, false) => ("_", "_"),
        (false, false, true) => ("~", "~"),
        (true, true, false) => ("*_", "_*"),
        (true, false, true) => ("*~", "~*"),
        (false, true, true) => ("_~", "~_"),
        (true, true, true) => ("*_~", "~_*"),
    }
}
