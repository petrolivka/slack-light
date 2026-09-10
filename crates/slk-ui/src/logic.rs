//! The decisions the window makes, with no widget in sight.
//!
//! Spike D3: relm4's `ComponentSender` cannot be constructed outside a
//! running component, so `update()` cannot be driven from a test. The
//! answer is not a test harness for relm4; it is to keep the decisions out
//! of `update()` in the first place. These are the ones the shell makes,
//! and `update()` calls them.

/// Which conversation to open first: one with a mention, else one with
/// anything unread, else the first. Muted ones never win a badge.
pub fn landing<'a>(convs: impl IntoIterator<Item = (&'a bool, &'a u32, &'a u32)>) -> usize {
    let convs: Vec<(bool, u32, u32)> = convs.into_iter().map(|(m, u, n)| (*m, *u, *n)).collect();
    convs
        .iter()
        .position(|(muted, _, mentions)| *mentions > 0 && !muted)
        .or_else(|| {
            convs
                .iter()
                .position(|(muted, unread, _)| *unread > 0 && !muted)
        })
        .unwrap_or(0)
}

/// Where an upserted message goes in the list.
#[derive(Debug, PartialEq, Eq)]
pub enum Placement {
    /// Over one row: the optimistic one it confirms, or itself being edited.
    Replace(usize),
    /// Two rows are the same message. Overwrite the earlier and delete the
    /// later, so it keeps the place it already had in the conversation.
    Merge {
        keep: usize,
        drop: usize,
    },
    Append,
}

/// FR-M4's "echo deduplication in both orders", as arithmetic.
///
/// A message sent from here can arrive back twice: once as the HTTP
/// response's confirmation, carrying `replaces` with the optimistic row's
/// local timestamp, and once as Slack's own websocket echo, carrying the real
/// timestamp and no `replaces` at all. Which lands first is a race, and Slack
/// frequently wins it.
///
/// Looking up only `replaces` handles one order. Looking up only `ts` handles
/// the other. Handling *both* means accepting that for a moment the list can
/// hold two rows for one message — the optimistic one and the echo — and that
/// the second answer has to collapse them rather than replace one and leave
/// the other. That is the case this returns `Merge` for, and the case that
/// put every threaded reply on screen twice.
pub fn placement(rows: &[&str], ts: &str, replaces: Option<&str>) -> Placement {
    let by_ts = rows.iter().position(|r| *r == ts);
    let by_replaces = replaces.and_then(|t| rows.iter().position(|r| *r == t));
    match (by_ts, by_replaces) {
        (Some(a), Some(b)) if a != b => Placement::Merge {
            keep: a.min(b),
            drop: a.max(b),
        },
        (Some(a), _) => Placement::Replace(a),
        (None, Some(b)) => Placement::Replace(b),
        (None, None) => Placement::Append,
    }
}

/// What a row shows besides its own message: whether it is grouped under
/// the one above, and which separators go over it.
///
/// Pure, and therefore tested. Grouping is what makes a conversation read
/// as a conversation rather than as a log, and getting it wrong is
/// invisible until someone writes twice in a row at midnight.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// Same author, close in time, same day: no avatar, no name, no time.
    pub grouped: bool,
    /// The day this message starts, when it is not the one above's.
    pub day_break: Option<String>,
    /// The first message the user has not read.
    pub unread_break: bool,
}

/// Seconds within which two messages from one author are one block. Slack
/// uses five minutes and it reads well.
const GROUP_WINDOW: i64 = 300;

pub fn meta(
    prev: Option<(&str, i64)>,
    author: &str,
    ts: i64,
    last_read: Option<i64>,
    day_label: impl Fn(i64) -> String,
) -> Meta {
    let new_day = match prev {
        Some((_, pts)) => day_label(pts) != day_label(ts),
        None => true,
    };
    let unread_break = match (last_read, prev) {
        // The first message after the read mark — and only if there is one
        // above it, since a conversation that opens entirely unread does not
        // need a line at the very top.
        (Some(lr), Some((_, pts))) => pts <= lr && ts > lr,
        _ => false,
    };
    Meta {
        grouped: !new_day
            && !unread_break
            && matches!(prev, Some((pa, pts)) if pa == author && ts - pts < GROUP_WINDOW),
        day_break: new_day.then(|| day_label(ts)),
        unread_break,
    }
}

/// The next row to select from the keyboard, clamped: alt-Down on the last
/// conversation stays there rather than wrapping, because wrapping is the
/// thing that sends a reply to the wrong channel.
pub fn step(selected: Option<usize>, len: usize, down: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (selected, down) {
        (None, _) => 0,
        (Some(i), true) => (i + 1).min(len - 1),
        (Some(i), false) => i.saturating_sub(1),
    })
}

/// Where the message cursor goes. Clamped at both ends, and from nowhere it
/// lands on the newest message, because that is where the eye already is.
pub fn cursor(at: Option<usize>, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len as isize - 1;
    Some(match at {
        None => last as usize,
        Some(i) => (i as isize + delta).clamp(0, last) as usize,
    })
}

/// Whether a chord may be installed as a window-wide accelerator.
///
/// The `slack` preset keeps the composer focused at all times, so a
/// modifier-less accelerator is not a shortcut — it is a character the user
/// can no longer type. The preset binds `[` and `]` to back and forward; as
/// accelerators those would eat every bracket typed into a code snippet. A
/// chord earns a global accelerator by having a modifier, or by being a key
/// that produces no text at all.
pub fn bindable(rendered: &str) -> bool {
    let mut parts = rendered.split('+').peekable();
    let mut modified = false;
    let mut shift = false;
    let mut key = "";
    while let Some(p) = parts.next() {
        match p {
            "ctrl" | "alt" if parts.peek().is_some() => modified = true,
            "shift" if parts.peek().is_some() => shift = true,
            other => key = other,
        }
    }
    // Shift on a character key never arrives. Measured through the Wayland
    // virtual keyboard: alt-shift-m is delivered as keyval `m` with
    // SHIFT|ALT, and neither `<Alt><Shift>m` nor `<Alt><Shift>M` matches it,
    // while `<Alt>m`, `<Alt>slash`, `<Alt>at` and `<Alt><Shift>Down` all fire.
    // Nine of the preset's bindings were dead this way with no warning from
    // anywhere. Shift on a *named* key — the arrows — is fine, so the rule is
    // about the character, not about shift.
    let one_char = key.chars().count() == 1;
    if (shift || key.chars().next().is_some_and(char::is_uppercase)) && one_char {
        return false;
    }
    if modified {
        return true;
    }
    key == "esc" || (key.starts_with('f') && key[1..].parse::<u8>().is_ok())
}

/// Whether an action can be taken on the message under the cursor, and what
/// to say when it cannot.
///
/// A silent no-op is the worst answer here: the user presses alt-e on
/// somebody else's message and cannot tell whether the key is unbound, the
/// message is unselected, or Slack refused. Every refusal has a sentence.
pub fn allowed(act: &str, mine: bool, files: usize, links: usize) -> Result<(), &'static str> {
    match act {
        "edit_message" if !mine => Err("you can only edit your own messages"),
        "delete_message" if !mine => Err("you can only delete your own messages"),
        "download_files" if files == 0 => Err("no files on this message"),
        "open_link" if links == 0 => Err("no link in this message"),
        _ => Ok(()),
    }
}

/// Apply a reaction toggle the way the server will, so the chip moves under
/// the pointer instead of a round trip later.
///
/// The engine deliberately does not echo a successful reaction — it only
/// re-fetches the message when one *fails* — so this is not a decoration on
/// top of the truth, it is the truth until something contradicts it.
pub fn toggle_reaction(reactions: &mut Vec<slk_core::Reaction>, name: &str) -> bool {
    match reactions.iter_mut().position(|r| r.name == name) {
        Some(i) => {
            let on = !reactions[i].by_me;
            reactions[i].by_me = on;
            if on {
                reactions[i].count += 1;
            } else {
                reactions[i].count = reactions[i].count.saturating_sub(1);
                if reactions[i].count == 0 {
                    reactions.remove(i);
                }
            }
            on
        }
        None => {
            reactions.push(slk_core::Reaction {
                name: name.to_string(),
                count: 1,
                by_me: true,
            });
            true
        }
    }
}

/// The first link in a message, for "open link".
///
/// Slack writes links in the body, in attachments and in Block Kit; this
/// takes the body's, which is what "the link in this message" means to
/// somebody looking at one.
pub fn first_link(doc: &slk_core::Doc) -> Option<String> {
    use slk_core::ast::BlockNode;
    use slk_core::Inline;
    fn scan(xs: &[Inline]) -> Option<String> {
        xs.iter().find_map(|i| match i {
            Inline::Link { url, .. } => Some(url.clone()),
            Inline::Date { url: Some(u), .. } => Some(u.clone()),
            _ => None,
        })
    }
    doc.0.iter().find_map(|b| match b {
        BlockNode::Section(xs) | BlockNode::Quote(xs) => scan(xs),
        BlockNode::List { items, .. } => items.iter().find_map(|i| scan(i)),
        BlockNode::Preformatted(_) => None,
    })
}

/// A rendered `slk-config` chord ("ctrl+k", "alt+up", "f1", "esc") as a
/// GTK accelerator ("<Control>k", "<Alt>Up", "F1", "Escape").
/// What the composer is completing, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Complete {
    User,
    Channel,
    Emoji,
    Command,
}

/// Where a completion starts and what has been typed into it.
#[derive(Debug, PartialEq, Eq)]
pub struct Completing {
    pub kind: Complete,
    /// Byte offset of the sigil in the text before the cursor.
    pub at: usize,
    pub query: String,
}

/// Read the text left of the cursor and decide what is being completed.
///
/// Deliberately strict, because the alternative is a popup that appears
/// while somebody is typing an e-mail address or a fraction:
///
/// - the sigil must start a word — `me@example` completes nothing
/// - the query may not contain whitespace; a space ends the completion
/// - `/` only counts at the very start of the message, which is what a
///   slash command is
/// - `:` needs at least one character after it, or every clock time opens
///   the emoji list
pub fn completing(before: &str) -> Option<Completing> {
    let sigil_at = before
        .char_indices()
        .rev()
        .find_map(|(i, c)| matches!(c, '@' | '#' | ':' | '/').then_some(i))?;
    let query = &before[sigil_at + 1..];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    let sigil = before[sigil_at..].chars().next()?;
    let prev = before[..sigil_at].chars().next_back();
    let boundary = prev.is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '[' | '{' | '"'));
    let kind = match sigil {
        '@' if boundary => Complete::User,
        '#' if boundary => Complete::Channel,
        ':' if boundary && !query.is_empty() => Complete::Emoji,
        '/' if sigil_at == 0 => Complete::Command,
        _ => return None,
    };
    Some(Completing {
        kind,
        at: sigil_at,
        query: query.to_string(),
    })
}

/// The slash commands worth offering. Slack has hundreds behind apps; these
/// are the ones the client itself can answer for, and anything else the user
/// types is passed through to Slack unchanged.
pub const COMMANDS: &[(&str, &str)] = &[
    ("/me", "Write in the third person"),
    ("/shrug", "Append ¯\\_(ツ)_/¯"),
    ("/topic", "Set the conversation's topic"),
    ("/purpose", "Set the conversation's purpose"),
    ("/remind", "Ask Slackbot to remind somebody"),
    ("/away", "Set yourself away"),
    ("/active", "Set yourself active"),
    ("/status", "Set your status: `:emoji: text`"),
    ("/dnd", "Snooze notifications for so many minutes"),
    ("/dm", "Open a direct message"),
    ("/msg", "Open a direct message"),
    ("/invite", "Invite somebody here"),
    ("/join", "Join or open a channel"),
    ("/leave", "Leave this conversation"),
    ("/create", "Create a channel: `/create name`"),
    ("/create-private", "Create a private channel"),
    ("/rename", "Rename this channel"),
    ("/archive", "Archive this channel, for everybody"),
    ("/group", "Start a conversation: `/group @alice @bob`"),
    ("/mute", "Mute this conversation"),
    ("/unmute", "Unmute this conversation"),
    ("/star", "Star this conversation"),
    ("/unstar", "Unstar this conversation"),
    ("/search", "Search Slack"),
    ("/upload", "Send a file"),
    ("/thread", "Open the selected message's thread"),
    ("/edit", "Edit the selected message"),
    ("/pins", "What is pinned here"),
];

/// What a conversation is called, given what the store holds and what the
/// user directory knows.
///
/// A direct message has no name of its own. `client.userBoot` puts them in
/// `ims`, which carry the other person's id and nothing else, so the name has
/// to come from the directory — and the directory usually arrives *after* the
/// conversations do, which is why this has to cope with not knowing yet.
///
/// The id is the last resort rather than an empty string: a row reading
/// `U0BV61H04S3` is at least a row you can click, and a bare presence dot
/// followed by nothing is what a real workspace looked like for a whole
/// milestone.
pub fn name_of(name: &str, peer: Option<&str>, from_directory: Option<&str>) -> String {
    if !name.is_empty() {
        return name.to_string();
    }
    from_directory
        .filter(|d| !d.is_empty())
        .or(peer)
        .unwrap_or("")
        .to_string()
}

/// The sections, by their `[sidebar] order` names, with their headings.
///
/// `custom` stands for every section the person made in Slack, in Slack's
/// order, and has no heading of its own — each of them has one. `recent` is a
/// view of the conversations most recently opened rather than a place one
/// belongs, so `section_key` never answers it.
pub const BUILT_IN_SECTIONS: [(&str, &str); 5] = [
    ("recent", "RECENT"),
    ("starred", "STARRED"),
    ("custom", ""),
    ("channels", "CHANNELS"),
    ("dms", "DIRECT MESSAGES"),
];

/// Which section a conversation is drawn in: `starred`, the id of one of the
/// person's own sections, `dms` or `channels`.
///
/// Starred first. A star given here moves the row at once, while Slack's
/// section list only catches up at the next boot; when both are fresh they
/// agree anyway, because Slack takes a starred conversation out of the
/// section it was in. Then a section the person made, then the built-in rule.
pub fn section_key(
    id: &slk_core::ChannelId,
    starred: bool,
    dm: bool,
    custom: &[slk_core::SidebarSection],
) -> String {
    if starred {
        return "starred".into();
    }
    if let Some(s) = custom.iter().find(|s| s.channels.contains(id)) {
        return s.id.clone();
    }
    if dm {
        "dms".into()
    } else {
        "channels".into()
    }
}

/// The sections to draw, in order, by key.
///
/// The built-ins where `[sidebar] order` puts them, and every section the
/// person made, in Slack's order, where `custom` stands. An unknown name is
/// ignored and a missing one appended, as before — so a config written
/// before M4, with no `custom` in it, gets the person's sections at the
/// bottom rather than not at all.
pub fn section_keys(order: &[String], custom: &[slk_core::SidebarSection]) -> Vec<String> {
    let mut names: Vec<&str> = Vec::new();
    for want in order {
        if let Some((n, _)) = BUILT_IN_SECTIONS.iter().find(|(n, _)| *n == want.as_str()) {
            if !names.contains(n) {
                names.push(n);
            }
        }
    }
    for (n, _) in BUILT_IN_SECTIONS {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let mut out = Vec::new();
    for n in names {
        if n == "custom" {
            out.extend(custom.iter().map(|s| s.id.clone()));
        } else {
            out.push(n.to_string());
        }
    }
    out
}

/// A section's heading: the built-in title, or the name the person gave
/// theirs — as they typed it, not upper-cased, because "iOS" is a name — with
/// its emoji in front when it is one this client can draw. A custom emoji is
/// an image a heading has no room for, and its name spelled out reads as
/// part of the heading.
pub fn section_title(key: &str, custom: &[slk_core::SidebarSection]) -> String {
    if let Some((_, t)) = BUILT_IN_SECTIONS.iter().find(|(n, _)| *n == key) {
        return t.to_string();
    }
    let Some(s) = custom.iter().find(|s| s.id == key) else {
        return String::new();
    };
    let name = if s.name.is_empty() {
        "untitled"
    } else {
        s.name.as_str()
    };
    match Some(s.emoji.as_str())
        .filter(|e| !e.is_empty())
        .and_then(|e| slk_core::emoji::shortcode(e, None))
    {
        Some(glyph) => format!("{glyph} {name}"),
        None => name.to_string(),
    }
}

/// How the sidebar's options rearrange one section's conversations.
///
/// Takes what each row is — its index, whether it has unread, whether it is
/// the one on screen — rather than the rows themselves, so it can be tested
/// without a sidebar.
///
/// `hide_read` never hides the open conversation. A sidebar that drops the
/// row you are reading the moment it is marked read is a sidebar that loses
/// your place while you watch.
pub fn arrange(
    rows: impl Iterator<Item = (usize, bool, bool)>,
    unread_first: bool,
    hide_read: bool,
) -> Vec<usize> {
    let mut kept: Vec<(usize, bool, bool)> = rows
        .filter(|(_, unread, open)| !hide_read || *unread || *open)
        .collect();
    if unread_first {
        // Stable, so within "unread" and within "read" the sidebar's own
        // order — most recent activity — still decides.
        kept.sort_by_key(|(_, unread, _)| !*unread);
    }
    kept.into_iter().map(|(i, _, _)| i).collect()
}

/// The first `n` lines of a snippet, and whether there are more.
///
/// Twelve by default: enough to see what a file is, few enough that three
/// snippets in a row do not become the whole conversation. Slack has usually
/// truncated it already, so this is the second cut, not the first.
pub fn snippet(preview: &str, n: usize) -> (String, usize) {
    let lines: Vec<&str> = preview.lines().collect();
    let shown = lines.len().min(n);
    (lines[..shown].join("\n"), lines.len().saturating_sub(shown))
}

/// Which of the three searches a query is asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Search {
    /// `search.messages`, with Slack's own modifiers.
    Slack,
    /// The local FTS index: instant, offline, and only what has been read.
    Local,
    /// `search.files`.
    Files,
}

/// Split a leading `local:` or `file:` off a query.
///
/// A prefix rather than a mode: the search box is one box, and a toggle you
/// cannot see the state of in a screenshot is a toggle people get wrong.
/// `from:bob` and the rest are Slack's and pass straight through — only these
/// two are ours, and only at the very start.
pub fn search_kind(query: &str) -> (Search, String) {
    let q = query.trim_start();
    for (prefix, kind) in [
        ("local:", Search::Local),
        ("file:", Search::Files),
        ("files:", Search::Files),
    ] {
        if let Some(rest) = q.strip_prefix(prefix) {
            return (kind, rest.trim_start().to_string());
        }
    }
    (Search::Slack, q.to_string())
}

/// "alice is typing…", for however many people are.
///
/// Names rather than a count, and at most two of them: "3 people are typing"
/// tells you nothing you wanted, and a line that grows with the room pushes
/// the composer around. Empty when nobody is, so the caller can fall back to
/// whatever the hint normally says.
pub fn typing_line(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => format!("{one} is typing…"),
        [a, b] => format!("{a} and {b} are typing…"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more are typing…", rest.len()),
    }
}

/// A message, as a quote to put in front of a reply.
///
/// Slack's own mrkdwn quote is a leading `>` per line, and a blank line ends
/// the block — so an empty line inside the quoted text has to be `>` too, or
/// the second half stops being a quote. The trailing newline is what puts the
/// cursor under the quote rather than inside it.
///
/// Attribution goes on the first line rather than after, because a quote whose
/// author is named underneath reads as the reply's own words on a narrow
/// window, and that is worth more than the two characters it costs.
pub fn quote(who: &str, text: &str) -> String {
    let mut out = String::new();
    if !who.is_empty() {
        out.push_str(&format!("> *{who}*\n"));
    }
    for line in text.lines() {
        out.push_str("> ");
        out.push_str(line);
        out.push('\n');
    }
    if text.is_empty() {
        out.push_str(">\n");
    }
    out.push('\n');
    out
}

/// A slash command the *interface* owns rather than the engine.
///
/// `/upload` opens a file chooser and `/search` focuses a box; neither has
/// anything to send. Returning the action name rather than doing the work
/// keeps one list of what each command means, which is what the command
/// palette and the shortcuts window both read.
///
/// The second half of the pair is the text to hand the action, if it takes
/// any: `/msg alice` should land in jump-to with `alice` already typed.
pub fn local(command: &str, text: &str) -> Option<(&'static str, String)> {
    let arg = text.trim().to_string();
    Some(match command {
        "/search" => ("search", arg),
        "/upload" => ("upload_file", String::new()),
        "/thread" => ("open_thread", String::new()),
        "/edit" => ("edit_message", String::new()),
        "/pins" => ("pinned", String::new()),
        // Archiving is the one act here everybody in the workspace sees
        // happen, and it has no undo in this client. It goes through the same
        // confirmation the key does, so typing it is not a shortcut past the
        // question.
        "/archive" => ("archive_channel", String::new()),
        // Jump-to already fuzzy-matches every conversation and person, so
        // `/msg alice` is that box with `alice` in it. Inventing a second
        // people-picker would give two answers to "who is alice".
        "/msg" | "/dm" => ("jump_to", arg),
        _ => return None,
    })
}

/// A message that is really a slash command, split into the two parts the
/// engine wants.
///
/// Only at the very start, and only a bare word: `/home/petr/notes` is a
/// path somebody is talking about, and `and/or` is a word. Slack forwards
/// anything shaped like a command to the workspace, so this does too —
/// what the workspace does not know comes back, and the interface puts the
/// text back in the composer rather than losing it.
pub fn slash(text: &str) -> Option<(String, String)> {
    let text = text.strip_prefix('/')?;
    let (word, rest) = match text.find(char::is_whitespace) {
        Some(i) => (&text[..i], text[i..].trim_start()),
        None => (text, ""),
    };
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some((format!("/{word}"), rest.to_string()))
}

/// Render `:shortcode:` as the glyph, for the one-line summaries the side
/// pane shows.
///
/// The lists carry Slack's raw text — that is what search returns and what
/// the store holds — so without this a result reads
/// "Deploy finished :white_check_mark:". The message body goes through the
/// full renderer instead; this is for a label.
pub fn readable(text: &str) -> String {
    if !text.contains(':') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find(':') {
            Some(end)
                if end > 0
                    && after[..end]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '+') =>
            {
                match slk_core::emoji::shortcode(&after[..end], None) {
                    Some(glyph) => out.push_str(&glyph),
                    // A custom emoji has no glyph anywhere; leave the name.
                    None => {
                        out.push(':');
                        out.push_str(&after[..end]);
                        out.push(':');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// When a conversation is marked read.
///
/// The requirements are blunt about this one: getting it wrong means
/// marking things read that the user never saw, and there is no way to find
/// them again afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkRead {
    /// Only when asked.
    Manual,
    /// As soon as the conversation is opened, if the window has focus.
    OnFocus,
    /// When the newest message is actually on screen, and the window has
    /// focus. The safest, and the slowest to clear a badge.
    OnView,
}

impl MarkRead {
    pub fn parse(s: &str) -> Self {
        match s {
            "manual" => MarkRead::Manual,
            "on_view" => MarkRead::OnView,
            _ => MarkRead::OnFocus,
        }
    }
}

/// Whether to mark the open conversation read right now.
///
/// `active` is whether the window has the keyboard: a client that clears
/// badges while it is buried behind a browser is a client that loses
/// messages.
pub fn should_mark(policy: MarkRead, active: bool, at_bottom: bool, just_opened: bool) -> bool {
    match policy {
        MarkRead::Manual => false,
        MarkRead::OnFocus => active && just_opened,
        MarkRead::OnView => active && at_bottom,
    }
}

/// Whether a notification the engine thought worth raising should actually
/// be raised.
///
/// The engine decides whether it *matters* — mentions, keywords, muting.
/// This decides whether the user can already see it, which is the half that
/// needs to know about windows.
pub fn should_notify(active: bool, is_open_conversation: bool, at_bottom: bool) -> bool {
    !(active && is_open_conversation && at_bottom)
}

/// A capital letter is written as `<Shift>` plus the lower-case key, which
/// is the form GTK parses — but see `bindable`: such a chord never actually
/// arrives, so nothing installs one. The translation is kept correct anyway,
/// because the shortcuts window renders it and a wrong string there is a lie
/// to the reader.
pub fn accel(rendered: &str) -> Option<String> {
    let mut mods = String::new();
    let mut shift = false;
    let mut key = None;
    for part in rendered.split('+') {
        match part {
            "ctrl" => mods.push_str("<Control>"),
            "alt" => mods.push_str("<Alt>"),
            "shift" => shift = true,
            "" => key = Some("plus".to_string()),
            k => {
                key = Some(match k {
                    "enter" => "Return".into(),
                    "esc" => "Escape".into(),
                    "tab" => "Tab".into(),
                    "space" => "space".into(),
                    "backspace" => "BackSpace".into(),
                    "up" | "down" | "left" | "right" | "home" | "end" | "delete" => {
                        let mut c = k.chars();
                        c.next().unwrap().to_uppercase().collect::<String>() + c.as_str()
                    }
                    "pageup" => "Page_Up".into(),
                    "pagedown" => "Page_Down".into(),
                    // GTK parses accelerators by key *name*: "<Alt>/" is not
                    // one, and gtk_accelerator_parse answers nothing rather
                    // than complaining, which is a binding that silently
                    // does not exist.
                    "/" => "slash".into(),
                    "[" => "bracketleft".into(),
                    "]" => "bracketright".into(),
                    "@" => "at".into(),
                    "," => "comma".into(),
                    "." => "period".into(),
                    ";" => "semicolon".into(),
                    "-" => "minus".into(),
                    "=" => "equal".into(),
                    f if f.starts_with('f') && f[1..].parse::<u8>().is_ok() => f.to_uppercase(),
                    c if c.chars().count() == 1 => {
                        let ch = c.chars().next().unwrap();
                        if ch.is_uppercase() {
                            shift = true;
                        }
                        ch.to_lowercase().to_string()
                    }
                    _ => return None,
                })
            }
        }
    }
    key.map(|k| format!("{mods}{}{k}", if shift { "<Shift>" } else { "" }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_direct_message_is_named_after_the_person_in_it() {
        use super::name_of;
        // A channel names itself.
        assert_eq!(name_of("fixtures", None, None), "fixtures");
        // A direct message does not: Slack's `ims` carry a user id and no
        // name at all, so it comes from the directory.
        assert_eq!(
            name_of("", Some("U0BV61H04S3"), Some("petr.olivka")),
            "petr.olivka"
        );
        // The directory arrives after the conversations do. Until it does,
        // the id — never an empty row, which is what this actually shipped as.
        assert_eq!(name_of("", Some("U0BV61H04S3"), None), "U0BV61H04S3");
        assert_eq!(name_of("", Some("U0BV61H04S3"), Some("")), "U0BV61H04S3");
    }

    #[test]
    fn hiding_read_conversations_never_hides_the_one_on_screen() {
        use super::arrange;
        // (index, has unread, is open)
        let rows = || [(0, false, false), (1, true, false), (2, false, true)].into_iter();
        assert_eq!(arrange(rows(), false, false), vec![0, 1, 2]);
        assert_eq!(
            arrange(rows(), false, true),
            vec![1, 2],
            "the open conversation stays even with nothing unread"
        );
        // Unread first, and stable within each group.
        assert_eq!(arrange(rows(), true, false), vec![1, 0, 2]);
    }

    #[test]
    fn a_search_prefix_is_ours_only_at_the_start() {
        use super::{search_kind, Search};
        assert_eq!(
            search_kind("local: deploy"),
            (Search::Local, "deploy".into())
        );
        assert_eq!(search_kind("file:report"), (Search::Files, "report".into()));
        // Slack's own modifiers pass through untouched, including one that
        // merely contains our word.
        assert_eq!(
            search_kind("from:bob has:link local"),
            (Search::Slack, "from:bob has:link local".into())
        );
        assert_eq!(
            search_kind("who ate the file:"),
            (Search::Slack, "who ate the file:".into())
        );
    }

    #[test]
    fn the_typing_line_names_people_rather_than_counting_them() {
        use super::typing_line;
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(typing_line(&n(&[])), "");
        assert_eq!(typing_line(&n(&["alice"])), "alice is typing…");
        assert_eq!(
            typing_line(&n(&["alice", "bob"])),
            "alice and bob are typing…"
        );
        assert_eq!(
            typing_line(&n(&["alice", "bob", "carol", "dave"])),
            "alice, bob and 2 more are typing…"
        );
    }

    #[test]
    fn a_quote_survives_the_blank_line_in_the_middle_of_it() {
        // A bare empty line ends a mrkdwn quote, so the second paragraph
        // would come out as the replier's own words — attributed to them, in
        // a conversation, which is the worst possible way to get it wrong.
        let q = super::quote("alice", "first\n\nsecond");
        assert_eq!(q, "> *alice*\n> first\n> \n> second\n\n");
        assert!(
            q.lines()
                .filter(|l| !l.is_empty())
                .all(|l| l.starts_with('>')),
            "every line of the quote is quoted: {q:?}"
        );
        // And the cursor lands under it, not in it.
        assert!(q.ends_with("\n\n"));
    }

    #[test]
    fn the_interface_owns_the_commands_with_nothing_to_send() {
        use super::local;
        assert_eq!(local("/upload", ""), Some(("upload_file", String::new())));
        assert_eq!(local("/msg", " alice "), Some(("jump_to", "alice".into())));
        assert_eq!(
            local("/search", "from:bob"),
            Some(("search", "from:bob".into()))
        );
        // Everything else is the engine's or the workspace's, and must not be
        // swallowed here: `/giphy` has to reach Slack.
        assert_eq!(local("/giphy", "cat"), None);
        assert_eq!(local("/topic", "x"), None);
        assert_eq!(local("/me", "waves"), None);
    }

    #[test]
    fn every_advertised_command_is_owned_by_somebody() {
        // The completion list is what the user is told exists. A command in
        // it that neither the interface nor the engine answers is a promise
        // the client does not keep — which is exactly how `/pins` was
        // advertised and then forwarded to Slack as an unknown command.
        const ENGINE: &[&str] = &[
            "/me",
            "/shrug",
            "/topic",
            "/purpose",
            "/away",
            "/active",
            "/status",
            "/dnd",
            "/invite",
            "/join",
            "/leave",
            "/mute",
            "/unmute",
            "/star",
            "/unstar",
            "/create",
            "/create-private",
            "/rename",
            "/group",
        ];
        // Slackbot's, not ours, and it works because the engine forwards
        // anything it does not recognise to the workspace.
        const PASS_THROUGH: &[&str] = &["/remind"];
        for (name, _) in super::COMMANDS {
            assert!(
                super::local(name, "").is_some()
                    || ENGINE.contains(name)
                    || PASS_THROUGH.contains(name),
                "{name} is offered but nothing answers it"
            );
        }
    }

    use super::*;

    #[test]
    fn landing_prefers_a_mention_then_unread_then_first() {
        let c = [(false, 0u32, 0u32), (false, 3, 0), (false, 1, 2)];
        assert_eq!(landing(c.iter().map(|(m, u, n)| (m, u, n))), 2);
        let c = [(false, 0u32, 0u32), (false, 3, 0), (false, 1, 0)];
        assert_eq!(landing(c.iter().map(|(m, u, n)| (m, u, n))), 1);
        let c = [(false, 0u32, 0u32), (false, 0, 0)];
        assert_eq!(landing(c.iter().map(|(m, u, n)| (m, u, n))), 0);
    }

    #[test]
    fn a_muted_conversation_never_wins() {
        let c = [(true, 5u32, 5u32), (false, 1, 0)];
        assert_eq!(landing(c.iter().map(|(m, u, n)| (m, u, n))), 1);
    }

    #[test]
    fn an_optimistic_row_is_replaced_in_place() {
        let rows = ["1.0", "2.0", "local-3"];
        assert_eq!(
            placement(&rows, "3.0", Some("local-3")),
            Placement::Replace(2)
        );
        assert_eq!(
            placement(&rows, "2.0", None),
            Placement::Replace(1),
            "an edit"
        );
        assert_eq!(placement(&rows, "4.0", None), Placement::Append);
        assert_eq!(placement(&rows, "4.0", Some("gone")), Placement::Append);
    }

    #[test]
    fn an_echo_that_beats_its_own_confirmation_does_not_double_the_message() {
        // FR-M4 says both orders. This is the one that was never handled: a
        // message sent from here comes back twice, and Slack's websocket echo
        // frequently arrives before the HTTP response does.
        //
        // Order A — confirmation first, then the echo:
        let rows = ["1.0", "local-3"];
        assert_eq!(
            placement(&rows, "3.0", Some("local-3")),
            Placement::Replace(1)
        );
        let after = ["1.0", "3.0"];
        assert_eq!(
            placement(&after, "3.0", None),
            Placement::Replace(1),
            "the echo lands on the row that is already there"
        );

        // Order B — the echo first, which appends beside the optimistic row,
        // and then the confirmation, which has to collapse the two rather
        // than replace one and leave the other. Leaving the other is what put
        // every threaded reply on screen twice.
        let rows = ["1.0", "local-3"];
        assert_eq!(placement(&rows, "3.0", None), Placement::Append);
        let both = ["1.0", "local-3", "3.0"];
        assert_eq!(
            placement(&both, "3.0", Some("local-3")),
            Placement::Merge { keep: 1, drop: 2 },
            "the message keeps the place it already had"
        );
    }

    #[test]
    fn stepping_clamps_at_both_ends() {
        assert_eq!(step(None, 3, true), Some(0));
        assert_eq!(step(Some(2), 3, true), Some(2));
        assert_eq!(step(Some(0), 3, false), Some(0));
        assert_eq!(step(Some(1), 3, false), Some(0));
        assert_eq!(step(Some(0), 0, true), None);
    }

    fn day(ts: i64) -> String {
        format!("day{}", ts / 86_400)
    }

    #[test]
    fn consecutive_messages_from_one_author_group() {
        let m = meta(Some(("alice", 1000)), "alice", 1100, None, day);
        assert!(m.grouped);
        assert_eq!(m.day_break, None);
    }

    #[test]
    fn a_different_author_breaks_the_group() {
        assert!(!meta(Some(("bob", 1000)), "alice", 1100, None, day).grouped);
    }

    #[test]
    fn a_long_gap_breaks_the_group() {
        assert!(!meta(Some(("alice", 1000)), "alice", 1000 + 301, None, day).grouped);
        assert!(meta(Some(("alice", 1000)), "alice", 1000 + 299, None, day).grouped);
    }

    #[test]
    fn the_first_message_is_never_grouped_and_always_starts_a_day() {
        let m = meta(None, "alice", 1000, None, day);
        assert!(!m.grouped);
        assert!(m.day_break.is_some());
    }

    #[test]
    fn a_day_boundary_breaks_the_group_even_for_one_author() {
        // Same author, one second apart, across midnight.
        let m = meta(Some(("alice", 86_399)), "alice", 86_400, None, day);
        assert!(!m.grouped, "a new day starts a new block");
        assert_eq!(m.day_break.as_deref(), Some("day1"));
    }

    #[test]
    fn the_unread_line_lands_on_the_first_unread_message_only() {
        // last_read at 1000: the message at 1100 is the first unread.
        assert!(meta(Some(("bob", 900)), "alice", 1100, Some(1000), day).unread_break);
        // The one after it is not.
        assert!(!meta(Some(("alice", 1100)), "alice", 1200, Some(1000), day).unread_break);
        // Nothing unread, no line.
        assert!(!meta(Some(("bob", 900)), "alice", 950, Some(1000), day).unread_break);
    }

    #[test]
    fn the_unread_line_breaks_the_group() {
        let m = meta(Some(("alice", 900)), "alice", 1100, Some(1000), day);
        assert!(m.unread_break);
        assert!(!m.grouped, "the line needs a name under it to make sense");
    }

    #[test]
    fn the_message_cursor_clamps_and_starts_at_the_newest() {
        assert_eq!(cursor(None, 10, -1), Some(9), "from nowhere, the newest");
        assert_eq!(cursor(Some(9), 10, 1), Some(9));
        assert_eq!(cursor(Some(0), 10, -1), Some(0));
        assert_eq!(cursor(Some(5), 10, -10), Some(0), "a page up past the top");
        assert_eq!(cursor(Some(5), 10, 10), Some(9));
        assert_eq!(cursor(None, 0, 1), None);
    }

    #[test]
    fn only_a_chord_that_is_not_text_becomes_an_accelerator() {
        assert!(bindable("ctrl+k"));
        assert!(bindable("alt+up"));
        assert!(bindable("f1"));
        assert!(bindable("esc"));
        // The preset binds these, and as accelerators they would eat the
        // bracket out of every code snippet typed into the composer.
        assert!(!bindable("["));
        assert!(!bindable("]"));
        assert!(!bindable("enter"));
        assert!(!bindable("tab"));
        // Shift on a character key is dead in GTK; shift on a named key
        // is not, and the difference is measured, not assumed.
        assert!(!bindable("alt+shift+m"));
        assert!(!bindable("alt+M"));
        assert!(!bindable("ctrl+shift+w"));
        assert!(bindable("alt+shift+down"));
        assert!(bindable("alt+slash"));
    }

    #[test]
    fn an_action_that_cannot_apply_says_why() {
        assert_eq!(allowed("edit_message", true, 0, 0), Ok(()));
        assert!(allowed("edit_message", false, 0, 0).is_err());
        assert!(allowed("delete_message", false, 0, 0).is_err());
        assert!(allowed("download_files", true, 0, 0).is_err());
        assert_eq!(allowed("download_files", true, 2, 0), Ok(()));
        assert!(allowed("open_link", true, 0, 0).is_err());
        // Anything not gated is allowed; the list is a set of exceptions,
        // not a permission table to keep in step with the action list.
        assert_eq!(allowed("react_1", false, 0, 0), Ok(()));
    }

    #[test]
    fn a_reaction_toggles_both_ways_and_the_last_one_removes_the_chip() {
        let mut r = Vec::new();
        assert!(toggle_reaction(&mut r, "tada"));
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].count, r[0].by_me), (1, true));

        // Off again: the chip goes with it, because a zero-count chip is a
        // reaction nobody made.
        assert!(!toggle_reaction(&mut r, "tada"));
        assert!(r.is_empty());

        // Joining somebody else's reaction leaves the chip and adds to it.
        let mut r = vec![slk_core::Reaction {
            name: "eyes".into(),
            count: 2,
            by_me: false,
        }];
        assert!(toggle_reaction(&mut r, "eyes"));
        assert_eq!((r[0].count, r[0].by_me), (3, true));
        assert!(!toggle_reaction(&mut r, "eyes"));
        assert_eq!((r[0].count, r[0].by_me), (2, false));
    }

    #[test]
    fn the_first_link_is_found_wherever_it_is_written() {
        use slk_core::ast::BlockNode;
        use slk_core::{Doc, Inline, Style};
        let text = |t: &str| Inline::Text {
            text: t.into(),
            style: Style::default(),
        };
        let link = |u: &str| Inline::Link {
            url: u.into(),
            text: None,
            style: Style::default(),
        };
        assert_eq!(first_link(&Doc(vec![])), None);
        assert_eq!(
            first_link(&Doc(vec![BlockNode::Section(vec![
                text("see "),
                link("https://a"),
                link("https://b"),
            ])]))
            .as_deref(),
            Some("https://a"),
        );
        // A code block is not a link, and a later section still is.
        assert_eq!(
            first_link(&Doc(vec![
                BlockNode::Preformatted("https://not-a-link".into()),
                BlockNode::Quote(vec![link("https://c")]),
            ]))
            .as_deref(),
            Some("https://c"),
        );
    }

    #[test]
    fn the_composer_completes_only_where_it_should() {
        assert_eq!(
            completing("hi @al"),
            Some(Completing {
                kind: Complete::User,
                at: 3,
                query: "al".into()
            })
        );
        assert_eq!(
            completing("see #des").map(|c| c.kind),
            Some(Complete::Channel)
        );
        assert_eq!(completing("yes :ro").map(|c| c.kind), Some(Complete::Emoji));
        assert_eq!(completing("/to").map(|c| c.kind), Some(Complete::Command));

        // An e-mail address is not a mention.
        assert_eq!(completing("me@example"), None);
        // A slash mid-sentence is a slash.
        assert_eq!(completing("and/or"), None);
        assert_eq!(completing("see foo/bar"), None);
        // A clock time is not an emoji.
        assert_eq!(completing("at 11:"), None);
        // A space closes it.
        assert_eq!(completing("hi @alice there"), None);
        assert_eq!(completing("plain words"), None);
        // An empty mention still completes: that is the moment the list of
        // everybody is most useful.
        assert_eq!(completing("hi @").map(|c| c.query), Some(String::new()));
    }

    #[test]
    fn a_bracket_counts_as_a_word_boundary() {
        assert_eq!(completing("(@al").map(|c| c.kind), Some(Complete::User));
        assert_eq!(completing("\"@al").map(|c| c.kind), Some(Complete::User));
    }

    #[test]
    fn marking_read_never_happens_behind_the_user_s_back() {
        use MarkRead::*;
        // Manual means manual.
        assert!(!should_mark(Manual, true, true, true));
        // On focus: opening it, with the window in front.
        assert!(should_mark(OnFocus, true, false, true));
        assert!(
            !should_mark(OnFocus, false, true, true),
            "window not in front"
        );
        assert!(!should_mark(OnFocus, true, true, false), "not just opened");
        // On view: the newest message actually on screen.
        assert!(should_mark(OnView, true, true, false));
        assert!(!should_mark(OnView, true, false, true), "scrolled back");
        assert!(!should_mark(OnView, false, true, true));
    }

    #[test]
    fn a_notification_is_skipped_only_when_it_is_already_on_screen() {
        // Looking straight at it.
        assert!(!should_notify(true, true, true));
        // Same conversation, but scrolled back through history.
        assert!(should_notify(true, true, false));
        // Another conversation, or another window.
        assert!(should_notify(true, false, true));
        assert!(should_notify(false, true, true));
    }

    #[test]
    fn the_mark_read_policy_names_are_the_configured_ones() {
        assert_eq!(MarkRead::parse("manual"), MarkRead::Manual);
        assert_eq!(MarkRead::parse("on_view"), MarkRead::OnView);
        assert_eq!(MarkRead::parse("on_focus"), MarkRead::OnFocus);
        // Anything unrecognised is the middle setting, not the riskiest one.
        assert_eq!(MarkRead::parse("nonsense"), MarkRead::OnFocus);
    }

    #[test]
    fn archiving_is_asked_about_however_it_is_asked_for() {
        // `/archive` is the interface's, so it reaches the same confirmation
        // dialog the key does. Were it the engine's, typing it would archive
        // a channel for everybody in it with no question at all.
        assert_eq!(
            local("/archive", ""),
            Some(("archive_channel", String::new()))
        );
        // The others take a name and go to the engine, where the directory
        // and the normalisation are.
        for c in ["/create", "/create-private", "/rename", "/group"] {
            assert_eq!(local(c, "x"), None, "{c} is the engine's");
            assert!(
                COMMANDS.iter().any(|(n, _)| *n == c),
                "{c} is offered by completion"
            );
        }
    }

    #[test]
    fn custom_sections_stand_where_custom_does_in_slacks_order() {
        use slk_core::{ChannelId, SectionKind, SidebarSection};
        let sec = |id: &str, name: &str, chans: &[&str]| SidebarSection {
            id: id.into(),
            name: name.into(),
            emoji: String::new(),
            kind: SectionKind::Custom,
            channels: chans.iter().map(|c| ChannelId::new(*c)).collect(),
        };
        let custom = [
            sec("L2", "Projects", &["C1"]),
            sec("L1", "Clients", &["C2"]),
        ];
        let order: Vec<String> = ["recent", "starred", "custom", "channels", "dms"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            section_keys(&order, &custom),
            ["recent", "starred", "L2", "L1", "channels", "dms"],
            "Slack's order among the custom ones, not sorted by id"
        );
        // A config written before M4 names no `custom`: appended, not lost.
        let old: Vec<String> = ["dms", "channels", "bogus"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            section_keys(&old, &custom),
            ["dms", "channels", "recent", "starred", "L2", "L1"]
        );
    }

    #[test]
    fn a_star_given_here_beats_a_section_slack_has_not_updated_yet() {
        use slk_core::{ChannelId, SectionKind, SidebarSection};
        let custom = [SidebarSection {
            id: "L2".into(),
            name: "Projects".into(),
            emoji: "rocket".into(),
            kind: SectionKind::Custom,
            channels: vec![ChannelId::new("C1"), ChannelId::new("D1")],
        }];
        let c1 = ChannelId::new("C1");
        assert_eq!(section_key(&c1, true, false, &custom), "starred");
        assert_eq!(section_key(&c1, false, false, &custom), "L2");
        // A direct message can be in a section the person made, too.
        assert_eq!(
            section_key(&ChannelId::new("D1"), false, true, &custom),
            "L2"
        );
        assert_eq!(
            section_key(&ChannelId::new("D9"), false, true, &custom),
            "dms"
        );
        assert_eq!(
            section_key(&ChannelId::new("C9"), false, false, &custom),
            "channels"
        );
        assert_eq!(section_title("L2", &custom), "🚀 Projects");
        assert_eq!(section_title("channels", &custom), "CHANNELS");
    }

    #[test]
    fn a_heading_never_spells_out_an_emoji_it_cannot_draw() {
        use slk_core::{SectionKind, SidebarSection};
        let custom = [SidebarSection {
            id: "L3".into(),
            name: "iOS".into(),
            emoji: "our-own-logo".into(),
            kind: SectionKind::Custom,
            channels: Vec::new(),
        }];
        assert_eq!(section_title("L3", &custom), "iOS", "and keeps its case");
    }

    #[test]
    fn a_slash_command_is_only_one_at_the_start_of_a_bare_word() {
        assert_eq!(
            slash("/topic release week"),
            Some(("/topic".into(), "release week".into()))
        );
        assert_eq!(slash("/shrug"), Some(("/shrug".into(), String::new())));
        // A path is not a command, and neither is a slash mid-sentence.
        assert_eq!(slash("/home/petr/notes.md is where"), None);
        assert_eq!(slash("and/or"), None);
        assert_eq!(slash("look at /etc/hosts"), None);
        assert_eq!(slash("/"), None);
        assert_eq!(slash("plain text"), None);
    }

    #[test]
    fn a_summary_shows_emoji_rather_than_their_names() {
        assert_eq!(readable("shipped :rocket:"), "shipped 🚀");
        assert_eq!(readable("no colons here"), "no colons here");
        // A time is not a shortcode, and neither is an unclosed one.
        assert_eq!(readable("at 11:30 sharp"), "at 11:30 sharp");
        assert_eq!(readable("ratio 3:1"), "ratio 3:1");
        assert_eq!(readable(":rocket"), ":rocket");
        // A workspace's own emoji has no glyph; the name stays.
        assert_eq!(readable("ship :partyparrot:"), "ship :partyparrot:");
        assert_eq!(readable(":+1: and :tada:"), "👍 and 🎉");
    }

    #[test]
    fn chords_become_accelerators() {
        assert_eq!(accel("ctrl+k").as_deref(), Some("<Control>k"));
        assert_eq!(accel("alt+up").as_deref(), Some("<Alt>Up"));
        assert_eq!(accel("f1").as_deref(), Some("F1"));
        assert_eq!(accel("esc").as_deref(), Some("Escape"));
        assert_eq!(accel("ctrl+shift+p").as_deref(), Some("<Control><Shift>p"));
        assert_eq!(accel("enter").as_deref(), Some("Return"));
        assert_eq!(accel("ctrl+1").as_deref(), Some("<Control>1"));
        assert_eq!(accel("alt+/").as_deref(), Some("<Alt>slash"));
        assert_eq!(accel("alt+[").as_deref(), Some("<Alt>bracketleft"));
        // The collision that cost an afternoon: GTK matches on the
        // lower-cased keyval, so a capital has to be written as shift.
        assert_eq!(accel("alt+T").as_deref(), Some("<Alt><Shift>t"));
        assert_ne!(accel("alt+T"), accel("alt+t"));
        assert_eq!(accel("nonsense"), None);
    }

    #[test]
    fn every_built_in_palette_parses_and_colours_the_css() {
        for text in [
            include_str!("../../slk-theme/themes/dark.toml"),
            include_str!("../../slk-theme/themes/light.toml"),
        ] {
            let p: slk_theme::Palette = toml::from_str(text).unwrap();
            let css = p.css();
            assert!(
                css.contains(&p.background),
                "background colour reaches the css"
            );
            assert!(css.contains(&p.accent));
            assert!(css.contains("@define-color sl_bg"));
        }
    }
}
