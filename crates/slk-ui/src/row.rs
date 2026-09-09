//! One message, as a recycled list row.
//!
//! `TypedListView` keeps a small pool of widgets and rebinds them as the
//! user scrolls, so `setup` builds the widget tree once per pooled row and
//! `bind` fills it per message. Everything expensive — markup, textures,
//! Block Kit trees — is created in `bind` and released in `unbind`, which is
//! what keeps five thousand rows the cost of thirty.
//!
//! The shape is the official client's — avatar, name, time, body indented
//! under them, consecutive messages from one person merged into a block —
//! drawn in the language of an editor: no line between messages, a mono
//! body, a rounded-square avatar, chips with a border rather than a fill.

use crate::app::Shared;
use crate::logic::Meta;
use crate::{blockkit, markup};
use gtk::prelude::*;
use relm4::typed_view::list::RelmListItem;
use slk_core::{Delivery, Message, Names};
use std::rc::Rc;

/// Where the body starts: the avatar's width plus its gap, so a grouped
/// message lines up under the one above it.
const GUTTER: i32 = 24 + 10;

pub struct Row {
    pub msg: Message,
    pub meta: Meta,
    pub shared: Rc<Shared>,
}

pub struct Widgets {
    /// The whole decoration above the message: day and unread separators.
    breaks: gtk::Box,
    /// Avatar and the name/time line, hidden entirely when grouped.
    head: gtk::Box,
    avatar: gtk::Label,
    author: gtk::Label,
    time: gtk::Label,
    body: gtk::Label,
    extras: gtk::Box,
    picture: gtk::Picture,
    chips: gtk::Box,
    thread: gtk::Label,
    /// The classes set last time, so `bind` can take them off again — a
    /// recycled widget keeps whatever the previous message put on it.
    classes: Vec<String>,
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

                #[name = "breaks"]
                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_visible: false,
                },

                #[name = "head"]
                gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_spacing: 10,
                    set_margin_start: 12,
                    set_margin_end: 12,
                    set_margin_top: 6,

                    #[name = "avatar"]
                    gtk::Label {
                        add_css_class: "avatar",
                        set_valign: gtk::Align::Start,
                    },

                    gtk::Box {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_spacing: 8,
                        set_valign: gtk::Align::Center,
                        #[name = "author"]
                        gtk::Label {
                            set_xalign: 0.0,
                        },
                        #[name = "time"]
                        gtk::Label {
                            set_xalign: 0.0,
                            add_css_class: "time",
                        },
                    },
                },

                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_spacing: 4,
                    set_margin_start: 12 + GUTTER,
                    set_margin_end: 12,
                    set_margin_bottom: 3,

                    #[name = "body"]
                    gtk::Label {
                        set_xalign: 0.0,
                        set_halign: gtk::Align::Fill,
                        set_hexpand: true,
                        set_use_markup: true,
                        set_wrap: true,
                        set_wrap_mode: gtk::pango::WrapMode::WordChar,
                        set_max_width_chars: 100,
                        set_selectable: true,
                        add_css_class: "body",
                    },
                    #[name = "extras"]
                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 6,
                    },
                    #[name = "picture"]
                    gtk::Picture {
                        set_halign: gtk::Align::Start,
                        set_valign: gtk::Align::Start,
                        set_hexpand: false,
                        set_can_shrink: true,
                        // `Fill`, not `Contain`: with `Contain` the height is
                        // computed from the width, the width depends on
                        // whether the scrollbar is showing, and the scrollbar
                        // depends on the height. On a conversation that
                        // happens to be about one screen tall that closes
                        // into a loop — measured at 60 Hz on an idle window,
                        // 1.9 % CPU and 45 MB of churned render nodes. The
                        // aspect is computed here instead, once, and the
                        // widget is told exactly how big it is.
                        set_content_fit: gtk::ContentFit::Fill,
                        set_visible: false,
                    },
                    #[name = "chips"]
                    gtk::Box {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_spacing: 5,
                        set_visible: false,
                    },
                    #[name = "thread"]
                    gtk::Label {
                        set_xalign: 0.0,
                        set_visible: false,
                        add_css_class: "threadlink",
                    },
                }
            }
        }
        (
            root,
            Widgets {
                breaks,
                head,
                avatar,
                author,
                time,
                body,
                extras,
                picture,
                chips,
                thread,
                classes: Vec::new(),
                file_id: None,
            },
        )
    }

    fn bind(&mut self, w: &mut Widgets, _root: &mut gtk::Box) {
        let m = &self.msg;
        let names = self.shared.names.borrow();
        let pal = self.shared.pal.borrow();
        let self_id = self.shared.self_id.borrow();
        let ctx = markup::Ctx {
            names: &*names,
            self_id: self_id.as_str(),
            pal: &pal,
        };

        // Separators, above everything.
        while let Some(c) = w.breaks.first_child() {
            w.breaks.remove(&c);
        }
        if let Some(day) = &self.meta.day_break {
            w.breaks.append(&rule(day, "daybreak", "hairline"));
        }
        if self.meta.unread_break {
            w.breaks
                .append(&rule("NEW MESSAGES", "newbreak", "newbreak-rule"));
        }
        w.breaks
            .set_visible(self.meta.day_break.is_some() || self.meta.unread_break);

        let who = match &m.author {
            slk_core::Author::User(id) => {
                names.user(id.as_str()).unwrap_or(id.as_str()).to_string()
            }
            slk_core::Author::Bot { name, .. } => name.clone(),
            slk_core::Author::System => "slackbot".to_string(),
        };

        // Avatar and name share one colour, chosen from the theme's accents
        // by the name, so the same person is the same colour everywhere.
        for c in w.classes.drain(..) {
            w.avatar.remove_css_class(&c);
            w.author.remove_css_class(&c);
        }
        if self.meta.grouped {
            w.head.set_visible(false);
        } else {
            w.head.set_visible(true);
            let slot = colour_slot(&who);
            let (a, n) = (format!("avatar-{slot}"), format!("name-{slot}"));
            w.avatar.add_css_class(&a);
            w.author.add_css_class(&n);
            w.classes.push(a);
            w.classes.push(n);

            let initial = who
                .chars()
                .find(|c| c.is_alphanumeric())
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into());
            w.avatar.set_label(&initial);

            let bot = matches!(m.author, slk_core::Author::Bot { .. });
            let mark = match &m.delivery {
                Delivery::Pending(_) => " ·",
                Delivery::Failed(_) => " ✗",
                Delivery::Confirmed => "",
            };
            w.author.set_markup(&format!(
                "<b>{}</b>{}{mark}",
                gtk::glib::markup_escape_text(&who),
                if bot { " <small>APP</small>" } else { "" }
            ));
            w.time.set_label(&hhmm(&m.ts));
        }

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
                "<span foreground=\"{}\">📎 {}</span>  <span foreground=\"{}\"><small>{}</small></span>",
                pal.link,
                gtk::glib::markup_escape_text(&f.name),
                pal.dim,
                human_size(f.size),
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
            let scale = (420.0 / pw as f64).min(1.0);
            let (sw, sh) = ((pw as f64 * scale) as i32, (ph as f64 * scale) as i32);
            // A size request as well as a paintable: `can_shrink` will
            // otherwise take a picture in a vertical box down to nothing,
            // which is exactly what it did — the texture was fetched, the
            // widget was in the tree, and the row showed a gap.
            w.picture.set_size_request(sw, sh);
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

        // Reactions as chips: bordered pills, the ones you are in filled with
        // the accent. Widgets rather than markup, so they can become buttons
        // in M2 without the row changing shape.
        while let Some(c) = w.chips.first_child() {
            w.chips.remove(&c);
        }
        for r in &m.reactions {
            let glyph = slk_core::emoji::shortcode(&r.name, None)
                .unwrap_or_else(|| format!(":{}:", r.name));
            let chip = gtk::Label::new(Some(&format!("{glyph} {}", r.count)));
            chip.add_css_class("chip");
            if r.by_me {
                chip.add_css_class("mine");
            }
            chip.set_tooltip_text(Some(&format!(":{}:", r.name)));
            w.chips.append(&chip);
        }
        w.chips.set_visible(!m.reactions.is_empty());

        if m.reply_count > 0 {
            let who = m
                .reply_users
                .iter()
                .map(|u| names.user(u.as_str()).unwrap_or(u.as_str()).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let last = m
                .latest_reply
                .as_ref()
                .map(|t| format!(" · last {}", hhmm(t)))
                .unwrap_or_default();
            w.thread.set_markup(&format!(
                "↳ {} repl{}{}{}",
                m.reply_count,
                if m.reply_count == 1 { "y" } else { "ies" },
                if who.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", gtk::glib::markup_escape_text(&who))
                },
                gtk::glib::markup_escape_text(&last),
            ));
            w.thread.set_visible(true);
        } else {
            w.thread.set_visible(false);
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
        while let Some(c) = w.chips.first_child() {
            w.chips.remove(&c);
        }
        while let Some(c) = w.breaks.first_child() {
            w.breaks.remove(&c);
        }
    }
}

/// A centred label between two hairlines — the day, or the read mark.
fn rule(text: &str, label_class: &str, rule_class: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_margin_start(12);
    row.set_margin_end(12);
    row.set_margin_top(12);
    row.set_margin_bottom(4);
    let line = |grow: bool| {
        let s = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        s.add_css_class(rule_class);
        s.set_valign(gtk::Align::Center);
        s.set_hexpand(grow);
        s.set_size_request(if grow { -1 } else { 28 }, 1);
        s
    };
    row.append(&line(false));
    let l = gtk::Label::new(Some(text));
    l.add_css_class(label_class);
    row.append(&l);
    row.append(&line(true));
    row
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

fn human_size(n: u64) -> String {
    const U: [&str; 4] = ["B", "kB", "MB", "GB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// Which of the theme's eight author colours this name gets. Stable, so the
/// same person is the same colour in every conversation and every theme.
pub fn colour_slot(name: &str) -> usize {
    let h = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    (h % 8) as usize
}
