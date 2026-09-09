//! One message, as a recycled list row.
//!
//! `TypedListView` keeps a small pool of widgets and rebinds them as the
//! user scrolls, so `setup` builds the widget tree once per pooled row and
//! `bind` fills it per message. Everything expensive — markup, textures,
//! Block Kit trees — is created in `bind` and released in `unbind`, which is
//! what keeps five thousand rows the cost of thirty.

use crate::app::Shared;
use crate::{blockkit, markup};
use gtk::prelude::*;
use relm4::typed_view::list::RelmListItem;
use slk_core::{Delivery, Message, Names};
use std::rc::Rc;

pub struct Row {
    pub msg: Message,
    pub shared: Rc<Shared>,
}

pub struct Widgets {
    author: gtk::Label,
    time: gtk::Label,
    body: gtk::Label,
    extras: gtk::Box,
    picture: gtk::Picture,
    reactions: gtk::Label,
    /// Which file this row's picture is showing or waiting for, so `unbind`
    /// can take it off the pending list.
    file_id: Option<String>,
}

impl RelmListItem for Row {
    type Root = gtk::Box;
    type Widgets = Widgets;

    fn setup(_item: &gtk::ListItem) -> (gtk::Box, Widgets) {
        relm4::view! {
            root = gtk::Box {
                set_orientation: gtk::Orientation::Vertical,
                set_spacing: 2,
                set_margin_start: 12,
                set_margin_end: 12,
                set_margin_top: 6,
                set_margin_bottom: 6,

                gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_spacing: 8,
                    #[name = "author"]
                    gtk::Label {
                        set_xalign: 0.0,
                        set_use_markup: true,
                    },
                    #[name = "time"]
                    gtk::Label {
                        set_xalign: 0.0,
                        set_use_markup: true,
                    },
                },
                #[name = "body"]
                gtk::Label {
                    set_xalign: 0.0,
                    set_halign: gtk::Align::Fill,
                    set_hexpand: true,
                    set_use_markup: true,
                    set_wrap: true,
                    set_wrap_mode: gtk::pango::WrapMode::WordChar,
                    set_max_width_chars: 110,
                    set_selectable: true,
                },
                #[name = "extras"]
                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_spacing: 6,
                },
                #[name = "picture"]
                gtk::Picture {
                    set_halign: gtk::Align::Start,
                    set_can_shrink: true,
                    set_content_fit: gtk::ContentFit::Contain,
                    set_visible: false,
                },
                #[name = "reactions"]
                gtk::Label {
                    set_xalign: 0.0,
                    set_use_markup: true,
                    set_visible: false,
                },
            }
        }
        (
            root,
            Widgets {
                author,
                time,
                body,
                extras,
                picture,
                reactions,
                file_id: None,
            },
        )
    }

    fn bind(&mut self, w: &mut Widgets, _root: &mut gtk::Box) {
        let m = &self.msg;
        let names = self.shared.names.borrow();
        let pal = self.shared.pal.borrow();
        let ctx = markup::Ctx {
            names: &*names,
            self_id: self.shared.self_id.as_str(),
            pal: &pal,
        };

        // Author, in a colour of their own; delivery state beside it.
        let who = match &m.author {
            slk_core::Author::User(id) => {
                names.user(id.as_str()).unwrap_or(id.as_str()).to_string()
            }
            slk_core::Author::Bot { name, .. } => format!("⚙ {name}"),
            slk_core::Author::System => "slackbot".to_string(),
        };
        let mark = match &m.delivery {
            Delivery::Pending(_) => " ⏳",
            Delivery::Failed(_) => " ✗",
            Delivery::Confirmed => "",
        };
        w.author.set_markup(&format!(
            "<span foreground=\"{}\"><b>{}</b></span>{mark}",
            author_colour(&who, &pal.authors),
            gtk::glib::markup_escape_text(&who)
        ));
        w.time.set_markup(&format!(
            "<span foreground=\"{}\"><small>{}</small></span>",
            pal.dim,
            hhmm(&m.ts)
        ));

        // Body: the AST as one label of markup.
        let mut body = markup::doc(&m.body, &ctx);
        if m.edited {
            body.push_str(&format!(
                " <span foreground=\"{}\"><small>(edited)</small></span>",
                pal.dim
            ));
        }
        w.body.set_visible(!body.is_empty());
        w.body.set_markup(&body);

        // Block Kit and files: real widgets, built per bind, dropped per unbind.
        while let Some(child) = w.extras.first_child() {
            w.extras.remove(&child);
        }
        if !m.blocks.is_empty() {
            w.extras.append(&blockkit::widgets(&m.blocks, &ctx));
        }
        for f in &m.files {
            if f.is_image() {
                continue;
            }
            let l = gtk::Label::new(None);
            l.set_use_markup(true);
            l.set_xalign(0.0);
            l.set_markup(&format!(
                "<span foreground=\"{}\">📎 {} ({} B)</span>",
                pal.link,
                gtk::glib::markup_escape_text(&f.name),
                f.size
            ));
            w.extras.append(&l);
        }

        // The first picture on the message: a placeholder of its final size
        // now, the texture when it arrives. The row asks for the fetch; the
        // engine does the I/O.
        w.file_id = None;
        w.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        w.picture.set_visible(false);
        if let Some(f) = m.files.iter().find(|f| f.is_image()) {
            let id = f.id.as_str().to_string();
            let (pw, ph) = (
                f.width.unwrap_or(320) as i32,
                f.height.unwrap_or(200) as i32,
            );
            let scale = (480.0 / pw as f64).min(1.0);
            let (sw, sh) = ((pw as f64 * scale) as i32, (ph as f64 * scale) as i32);
            w.picture.set_visible(true);
            if let Some(t) = self.shared.textures.borrow().get(&id) {
                w.picture.set_paintable(Some(t));
            } else {
                // An empty paintable of the final size, not `set_size_request`:
                // a size request is a *minimum*, and a Picture with no
                // paintable has a natural size of zero, which GTK rightly
                // reports as "natural must be >= min" on every row. A
                // placeholder paintable gives it the right natural size and
                // lets `can_shrink` do its job.
                let blank = gtk::gdk::Paintable::new_empty(sw, sh);
                w.picture.set_paintable(Some(&blank));
                self.shared
                    .pending
                    .borrow_mut()
                    .entry(id.clone())
                    .or_default()
                    .push(w.picture.clone());
                if let Some(url) = &f.url_private {
                    self.shared.need_image(&id, url);
                }
            }
            w.file_id = Some(id);
        }

        // Reactions as chips of text; the real client makes them buttons.
        if m.reactions.is_empty() && m.reply_count == 0 {
            w.reactions.set_visible(false);
        } else {
            let mut parts: Vec<String> = m
                .reactions
                .iter()
                .map(|r| {
                    let glyph = slk_core::emoji::shortcode(&r.name, None)
                        .unwrap_or_else(|| format!(":{}:", r.name));
                    let bg = if r.by_me {
                        pal.link.as_str()
                    } else {
                        pal.code_bg.as_str()
                    };
                    format!(
                        "<span background=\"{bg}\" background_alpha=\"25%\"> {} {} </span>",
                        gtk::glib::markup_escape_text(&glyph),
                        r.count
                    )
                })
                .collect();
            if m.reply_count > 0 {
                parts.push(format!(
                    "<span foreground=\"{}\">↳ {} repl{}</span>",
                    pal.mention,
                    m.reply_count,
                    if m.reply_count == 1 { "y" } else { "ies" }
                ));
            }
            w.reactions.set_markup(&parts.join("  "));
            w.reactions.set_visible(true);
        }
    }

    fn unbind(&mut self, w: &mut Widgets, _root: &mut gtk::Box) {
        // Release the texture and the Block Kit tree: a row that scrolled
        // away must cost nothing, or NFR-4 is a lie.
        w.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        if let Some(id) = w.file_id.take() {
            if let Some(list) = self.shared.pending.borrow_mut().get_mut(&id) {
                list.retain(|p| p != &w.picture);
            }
        }
        while let Some(child) = w.extras.first_child() {
            w.extras.remove(&child);
        }
    }
}

fn hhmm(ts: &slk_core::Ts) -> String {
    jiff::Timestamp::from_second(ts.secs())
        .ok()
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// A stable colour per author, from the theme's own accents, so the same
/// name is the same colour in every row and every theme.
fn author_colour<'a>(name: &str, palette: &'a [String; 8]) -> &'a str {
    let h = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    &palette[(h % 8) as usize]
}
