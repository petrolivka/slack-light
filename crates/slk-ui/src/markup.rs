//! The rich-text AST as Pango markup.
//!
//! This is the whole of what `slk-render` was for, in a fraction of the
//! lines: no widths, no wrapping, no measured tables. Pango lays the text
//! out; this only says which run is bold, which is a link, which is a name.
//! Every piece of text passes through `markup_escape_text`, because a message
//! that says `<b>` must show `<b>`.

use gtk::glib;
use slk_core::ast::{BlockNode, Doc, Inline, ListStyle, Style};
use slk_core::Names;

pub use slk_theme::Semantic as Palette;

pub struct Ctx<'a> {
    pub names: &'a dyn Names,
    pub self_id: &'a str,
    pub pal: &'a Palette,
}

fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

fn styled(text: &str, style: Style, ctx: &Ctx) -> String {
    let mut open = String::new();
    let mut close = String::new();
    if style.code {
        // A translucent background rather than a colour: readable on the
        // dark theme and the light one alike, until spike B makes the
        // palette the theme's.
        open.push_str(&format!(
            "<span font_family=\"monospace\" background=\"{}\" background_alpha=\"20%\">",
            ctx.pal.code_bg
        ));
        close.insert_str(0, "</span>");
    }
    if style.bold {
        open.push_str("<b>");
        close.insert_str(0, "</b>");
    }
    if style.italic {
        open.push_str("<i>");
        close.insert_str(0, "</i>");
    }
    if style.strike {
        open.push_str("<s>");
        close.insert_str(0, "</s>");
    }
    format!("{open}{}{close}", esc(text))
}

pub fn inlines(xs: &[Inline], ctx: &Ctx) -> String {
    let mut out = String::new();
    for x in xs {
        match x {
            Inline::Text { text, style } => out.push_str(&styled(text, *style, ctx)),
            Inline::Link { url, text, style } => {
                let label = text.as_deref().unwrap_or(url);
                out.push_str(&format!(
                    "<a href=\"{}\"><span foreground=\"{}\">{}</span></a>",
                    esc(url),
                    ctx.pal.link,
                    styled(label, *style, ctx)
                ));
            }
            Inline::User { id, label } => {
                let name = ctx
                    .names
                    .user(id)
                    .map(str::to_string)
                    .or_else(|| label.clone())
                    .unwrap_or_else(|| id.clone());
                // Your own name is the one the eye is looking for.
                if id == ctx.self_id {
                    out.push_str(&format!(
                        "<span background=\"{}\" foreground=\"{}\"><b>@{}</b></span>",
                        ctx.pal.mention_self_bg,
                        ctx.pal.mention,
                        esc(&name)
                    ));
                } else {
                    out.push_str(&format!(
                        "<span foreground=\"{}\">@{}</span>",
                        ctx.pal.mention,
                        esc(&name)
                    ));
                }
            }
            Inline::UserGroup { id, label } => {
                let name = ctx
                    .names
                    .usergroup(id)
                    .map(str::to_string)
                    .or_else(|| label.clone())
                    .unwrap_or_else(|| id.clone());
                out.push_str(&format!(
                    "<span foreground=\"{}\">@{}</span>",
                    ctx.pal.mention,
                    esc(name.trim_start_matches('@'))
                ));
            }
            Inline::Channel { id, label } => {
                let name = ctx
                    .names
                    .channel(id)
                    .map(str::to_string)
                    .or_else(|| label.clone())
                    .unwrap_or_else(|| id.clone());
                out.push_str(&format!(
                    "<span foreground=\"{}\">#{}</span>",
                    ctx.pal.link,
                    esc(&name)
                ));
            }
            Inline::Broadcast(b) => out.push_str(&format!(
                "<span background=\"{}\" foreground=\"{}\"><b>@{}</b></span>",
                ctx.pal.mention_self_bg,
                ctx.pal.mention,
                b.as_str()
            )),
            Inline::Emoji {
                name,
                skin,
                unicode,
            } => {
                let text = unicode
                    .clone()
                    .or_else(|| slk_core::emoji::shortcode(name, *skin))
                    .unwrap_or_else(|| format!(":{name}:"));
                out.push_str(&esc(&text));
            }
            Inline::Date { fallback, .. } => out.push_str(&format!(
                "<span foreground=\"{}\">{}</span>",
                ctx.pal.dim,
                esc(fallback)
            )),
            Inline::Break => out.push('\n'),
        }
    }
    out
}

/// One label's worth of markup for a whole document. Paragraph-level
/// structure — quotes, lists, code blocks — is expressed in-line, because a
/// row is one label and one label is what keeps five thousand rows cheap.
pub fn doc(d: &Doc, ctx: &Ctx) -> String {
    let mut out = String::new();
    for (i, node) in d.0.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match node {
            BlockNode::Section(xs) => out.push_str(&inlines(xs, ctx)),
            BlockNode::Quote(xs) => {
                let body = inlines(xs, ctx);
                for (j, line) in body.split('\n').enumerate() {
                    if j > 0 {
                        out.push('\n');
                    }
                    out.push_str(&format!(
                        "<span foreground=\"{}\">┃</span> <i>{line}</i>",
                        ctx.pal.dim
                    ));
                }
            }
            BlockNode::Preformatted(code) => out.push_str(&format!(
                "<span font_family=\"monospace\" background=\"{}\" background_alpha=\"20%\">{}</span>",
                ctx.pal.code_bg,
                esc(code)
            )),
            BlockNode::List {
                style,
                indent,
                items,
            } => {
                let pad = "    ".repeat(*indent as usize);
                for (k, item) in items.iter().enumerate() {
                    if k > 0 {
                        out.push('\n');
                    }
                    let marker = match style {
                        ListStyle::Bullet => "•".to_string(),
                        ListStyle::Ordered => format!("{}.", k + 1),
                    };
                    out.push_str(&format!("{pad}{marker} {}", inlines(item, ctx)));
                }
            }
        }
    }
    out
}
