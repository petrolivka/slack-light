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
//!
//! Everything the row can *do* goes through one message, `Msg::RowAction`,
//! carrying a timestamp and an action name from `slk-config`. The pointer and
//! the keyboard therefore run the same code: clicking the thread link and
//! pressing alt-t are the same call, and an action can only be wrong in one
//! place. It also means the hover bar needs no state of its own — `bind`
//! writes the current timestamp into a cell the buttons close over, which is
//! the only way a recycled widget can know which message it is showing.

use crate::app::Shared;
use crate::logic::Meta;
use crate::{blockkit, markup};
use gtk::prelude::*;
use relm4::typed_view::list::RelmListItem;
use slk_core::{Delivery, Message, Names};
use std::cell::RefCell;
use std::rc::Rc;

/// What the "more" menu offers, in the order the official client offers it.
/// Every entry is an action name, so the menu, the keyboard and the command
/// palette cannot drift apart.
const MORE: &[(&str, &str)] = &[
    ("Edit message", "edit_message"),
    ("Delete message", "delete_message"),
    ("Quote in a reply", "quote"),
    ("Forward…", "forward_message"),
    ("Copy text", "copy_text"),
    ("Copy link", "copy_link"),
    ("Open first link", "open_link"),
    ("Save for later", "save"),
    ("Pin to conversation", "pin"),
    ("View image", "view_image"),
    ("Download files", "download_files"),
    // Last, and only when `[debug] enabled` is on. The window says why when
    // it is not, rather than the menu silently offering nothing.
    ("View source", "view_source"),
];

/// A text file, as its first lines with the rest behind a disclosure.
///
/// No syntax colouring. Slack sends `preview_highlight` as a block of HTML
/// from its own editor, and turning that into Pango means either shipping an
/// HTML parser or writing a highlighter — and a highlighter that is wrong
/// about a language is worse than monospace, because it asserts things about
/// code that are not true. Monospace, the code-block styling this client
/// already has, and the filetype named in the header instead.
fn snippet(f: &slk_core::FileMeta, pal: &slk_theme::Semantic) -> gtk::Widget {
    use gtk::prelude::*;
    let preview = f.preview.clone().unwrap_or_default();
    let (head, more) = crate::logic::snippet(&preview, 12);

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 2);
    outer.add_css_class("snippet");

    let title = gtk::Label::new(None);
    title.set_use_markup(true);
    title.set_xalign(0.0);
    let kind = if f.filetype.is_empty() {
        String::new()
    } else {
        format!("  ·  {}", f.filetype)
    };
    let count = match f.lines {
        Some(n) => format!("  ·  {n} lines"),
        None => String::new(),
    };
    title.set_markup(&format!(
        "<span foreground=\"{}\">📄 {}</span><span foreground=\"{}\"><small>{}{}  ·  {}</small></span>",
        pal.link,
        gtk::glib::markup_escape_text(&f.name),
        pal.dim,
        gtk::glib::markup_escape_text(&kind),
        gtk::glib::markup_escape_text(&count),
        human_size(f.size),
    ));
    outer.append(&title);

    let body = gtk::Label::new(Some(&head));
    body.set_xalign(0.0);
    body.set_selectable(true);
    body.set_wrap(false);
    body.add_css_class("code");
    outer.append(&body);

    // The rest of what Slack sent, not the rest of the file: expanding must
    // not become a download, and saying so is better than pretending the
    // whole file is here.
    if more > 0 {
        let rest = gtk::Label::new(Some(&preview));
        rest.set_xalign(0.0);
        rest.set_selectable(true);
        rest.set_wrap(false);
        rest.add_css_class("code");
        let expander = gtk::Expander::new(Some(&format!("{more} more lines")));
        expander.set_child(Some(&rest));
        outer.append(&expander);
    }
    if f.lines
        .is_some_and(|n| n as usize > preview.lines().count())
    {
        let note = gtk::Label::new(Some("· the rest is in the file"));
        note.set_xalign(0.0);
        note.add_css_class("note");
        outer.append(&note);
    }
    outer.upcast()
}

/// Run an action against one message.
///
/// A pooled button cannot hold a `ComponentSender` — it outlives every model
/// it was built for — so the window leaves one where any widget on the main
/// thread can reach it. Same call as the keyboard's, by design.
fn send(at: &str, act: &str) {
    crate::app::row_action(at, act);
}

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
    /// The hover bar, and the timestamp its buttons act on. A pooled widget
    /// outlives the message it was bound to, so the buttons cannot capture
    /// one; they read this instead.
    actions: gtk::Box,
    at: Rc<RefCell<String>>,
    /// The image on this row, for the click that opens it.
    file_of: Rc<RefCell<Option<String>>>,
    marks: gtk::Label,
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
    type Root = gtk::Overlay;
    type Widgets = Widgets;

    fn setup(item: &gtk::ListItem) -> (gtk::Overlay, Widgets) {
        // The shared handle is on the item's own data only after the first
        // bind, so the buttons are wired to a sender taken from the list
        // item's root at click time instead. See `send`.
        let _ = item;
        relm4::view! {
            content = gtk::Box {
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

                    // Pinned and saved go here rather than on the name
                    // line, because a grouped message has no name line — and
                    // a pin you cannot see is a pin you will not remove.
                    #[name = "marks"]
                    gtk::Label {
                        set_xalign: 0.0,
                        add_css_class: "marks",
                        set_visible: false,
                    },
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
                        // widget is told exactly how tall it is. `Contain`
                        // is safe *because* the height is fixed: the loop
                        // was height-from-width, and there is none now.
                        set_content_fit: gtk::ContentFit::Contain,
                        set_halign: gtk::Align::Start,
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
        let at = Rc::new(RefCell::new(String::new()));
        // Which image this pooled row is currently showing, for the click
        // handler — the same problem `at` solves for the action bar.
        let file_of: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        // The hover bar. An overlay rather than a row of its own, so nothing
        // moves when it appears: a bar that reflows the message under the
        // pointer is a bar you cannot click.
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        actions.add_css_class("msgactions");
        actions.set_halign(gtk::Align::End);
        actions.set_valign(gtk::Align::Start);
        actions.set_margin_end(20);
        actions.set_visible(false);

        // Built on the first hover, not per pooled row. Five buttons and a
        // menu on thirty pooled rows is a hundred and eighty widgets, and
        // most rows are never pointed at. Measured: the bar cost 15 MB of
        // private memory when it was built eagerly.
        let build = |bar: &gtk::Box, at: &Rc<RefCell<String>>| {
            let button = |label: &str, tip: &str, act: &str, at: &Rc<RefCell<String>>| {
                let b = gtk::Button::new();
                b.set_child(Some(&gtk::Label::new(Some(label))));
                b.set_tooltip_text(Some(tip));
                b.add_css_class("flat");
                // The composer keeps the keyboard: an action bar that takes
                // focus costs a click to give it back, every time.
                b.set_can_focus(false);
                let at = at.clone();
                let act = act.to_string();
                b.connect_clicked(move |_| send(&at.borrow(), &act));
                b
            };

            for (i, name) in slk_core::emoji::QUICK.iter().take(3).enumerate() {
                let glyph = slk_core::emoji::shortcode(name, None).unwrap_or_else(|| "+".into());
                bar.append(&button(
                    &glyph,
                    &format!(":{name}:"),
                    &format!("react_{}", i + 1),
                    at,
                ));
            }
            bar.append(&button("☺", "Add a reaction", "react", at));
            bar.append(&button("↳", "Reply in thread", "open_thread", at));

            let more = gtk::MenuButton::new();
            // A child, not `set_label`: a labelled MenuButton draws a
            // dropdown arrow next to it, and a second glyph in a six-button
            // bar reads as a seventh button.
            more.set_child(Some(&gtk::Label::new(Some("⋯"))));
            more.set_tooltip_text(Some("More actions"));
            more.add_css_class("flat");
            more.set_can_focus(false);
            let at2 = at.clone();
            // The menu itself is built when it is opened, for the same
            // reason the bar is.
            more.set_create_popup_func(move |mb| {
                let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
                list.add_css_class("moremenu");
                let pop = gtk::Popover::new();
                for (label, act) in MORE {
                    let b = gtk::Button::new();
                    b.set_child(Some(&{
                        let l = gtk::Label::new(Some(label));
                        l.set_xalign(0.0);
                        l
                    }));
                    b.add_css_class("flat");
                    let at = at2.clone();
                    let act = act.to_string();
                    let pop = pop.clone();
                    b.connect_clicked(move |_| {
                        pop.popdown();
                        send(&at.borrow(), &act);
                    });
                    list.append(&b);
                }
                pop.set_child(Some(&list));
                mb.set_popover(Some(&pop));
            });
            bar.append(&more);
            more
        };

        let root = gtk::Overlay::new();
        root.set_child(Some(&content));
        root.add_overlay(&actions);

        // Hover reveals it, and builds it the first time. Not hidden again
        // while the "more" menu is open, or walking to the menu would close
        // the menu.
        {
            let motion = gtk::EventControllerMotion::new();
            let menu: Rc<RefCell<Option<gtk::MenuButton>>> = Rc::new(RefCell::new(None));
            let (bar, at2, m) = (actions.clone(), at.clone(), menu.clone());
            motion.connect_enter(move |_, _, _| {
                if m.borrow().is_none() {
                    *m.borrow_mut() = Some(build(&bar, &at2));
                }
                bar.set_visible(true);
            });
            let (bar2, m2) = (actions.clone(), menu.clone());
            motion.connect_leave(move |_| {
                if !m2.borrow().as_ref().is_some_and(|mb| mb.is_active()) {
                    bar2.set_visible(false);
                }
            });
            root.add_controller(motion);
        }

        // An image opens at its own size. A click, not a button: the picture
        // is the affordance, which is what every other client has taught
        // people to expect.
        {
            let click = gtk::GestureClick::new();
            let at = at.clone();
            let shared_file = file_of.clone();
            click.connect_released(move |_, _, _, _| {
                let _ = &at;
                if let Some(id) = shared_file.borrow().as_ref() {
                    crate::app::image_clicked(id);
                }
            });
            picture.add_controller(click);
            picture.set_cursor_from_name(Some("zoom-in"));
        }

        // Links in the body are the window's to route: a Slack permalink is
        // a jump inside the client, and everything else goes to the browser.
        body.connect_activate_link(|_, url| {
            if crate::app::link_clicked(url) {
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });

        // The thread summary is a link: clicking it opens the thread, which
        // is what everyone tries first.
        {
            let click = gtk::GestureClick::new();
            let at = at.clone();
            click.connect_released(move |_, _, _, _| send(&at.borrow(), "open_thread"));
            thread.add_controller(click);
            thread.set_cursor_from_name(Some("pointer"));
        }

        (
            root,
            Widgets {
                breaks,
                actions,
                at,
                file_of,
                head,
                avatar,
                author,
                time,
                marks,
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

    fn bind(&mut self, w: &mut Widgets, _root: &mut gtk::Overlay) {
        let m = &self.msg;
        let names = self.shared.names.borrow();
        let pal = self.shared.pal.borrow();
        let self_id = self.shared.self_id.borrow();
        let ctx = markup::Ctx {
            names: &*names,
            self_id: self_id.as_str(),
            pal: &pal,
        };

        // What every button on this row will act on. First, because a
        // recycled row is still wired to the last message until this runs.
        *w.at.borrow_mut() = m.ts.as_str().to_string();
        w.actions.set_visible(false);

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
        let marks = match (m.pinned, m.saved) {
            (true, true) => "📌 pinned · 🔖 saved",
            (true, false) => "📌 pinned",
            (false, true) => "🔖 saved",
            (false, false) => "",
        };
        w.marks.set_label(marks);
        w.marks.set_visible(!marks.is_empty());

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
            // A text file shows what is in it. A paperclip with a name tells
            // you nothing about a two-line config change, and the whole point
            // of pasting a snippet is that people read it without opening it.
            if f.is_snippet() {
                w.extras.append(&snippet(f, &pal));
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
        *w.file_of.borrow_mut() = None;
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
            // Height only. A width request is a *minimum* for the whole
            // row, and the list's minimum is the widest row in it: with a
            // 420-pixel picture asking for its width, opening the side pane
            // made the conversation narrower than its own rows and every
            // message was clipped behind the sidebar. The height alone is
            // enough to stop `can_shrink` taking the picture to nothing,
            // which is what the request was for.
            w.picture.set_size_request(-1, sh);
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
            *w.file_of.borrow_mut() = Some(id.clone());
            w.file_id = Some(id);
        }

        // Reactions as chips: bordered pills, the ones you are in filled with
        // the accent. Widgets rather than markup, so they can become buttons
        // in M2 without the row changing shape.
        while let Some(c) = w.chips.first_child() {
            w.chips.remove(&c);
        }
        for r in &m.reactions {
            let chip = gtk::Button::new();
            // A workspace's own emoji is an image, and a chip is a widget, so
            // this is the one place FR-F6 costs nothing: a Box with a Picture
            // in it. The message *body* is a single Pango label — that is
            // what makes selection and copy work across a whole message — and
            // a label cannot hold an image without giving that up, so custom
            // shortcodes still read as `:name:` there.
            match self.shared.custom.borrow().get(&r.name) {
                Some(url) => {
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                    let pic = gtk::Picture::new();
                    pic.set_size_request(16, 16);
                    pic.set_content_fit(gtk::ContentFit::Contain);
                    // Without this the chip's only accessible name is its
                    // count — "2" — which tells a screen reader nothing and
                    // makes the suite unable to see which reaction it is.
                    // CONTRIBUTING: a widget that is not in the tree with a
                    // usable name is a defect twice over.
                    pic.update_property(&[gtk::accessible::Property::Label(&format!(
                        ":{}:",
                        r.name
                    ))]);
                    let key = format!("emoji:{}", r.name);
                    match self.shared.textures.borrow().get(&key) {
                        Some(t) => pic.set_paintable(Some(t)),
                        None => {
                            self.shared
                                .pending
                                .borrow_mut()
                                .entry(key.clone())
                                .or_default()
                                .push(pic.clone());
                            self.shared.need_image(&key, url);
                        }
                    }
                    row.append(&pic);
                    row.append(&gtk::Label::new(Some(&r.count.to_string())));
                    chip.set_child(Some(&row));
                }
                None => {
                    let glyph = slk_core::emoji::shortcode(&r.name, None)
                        .unwrap_or_else(|| format!(":{}:", r.name));
                    chip.set_child(Some(&gtk::Label::new(Some(&format!(
                        "{glyph} {}",
                        r.count
                    )))));
                }
            }
            chip.add_css_class("chip");
            chip.set_can_focus(false);
            if r.by_me {
                chip.add_css_class("mine");
            }
            chip.set_tooltip_text(Some(&format!(
                ":{}: — click to {}",
                r.name,
                if r.by_me { "remove yours" } else { "add yours" }
            )));
            let at = w.at.clone();
            let name = r.name.clone();
            chip.connect_clicked(move |_| send(&at.borrow(), &format!("react:{name}")));
            w.chips.append(&chip);
        }
        // One more, to add a reaction where there already are some: the row
        // where reactions live is where people look for the button.
        if !m.reactions.is_empty() {
            let add = gtk::Button::new();
            add.set_child(Some(&gtk::Label::new(Some("＋"))));
            add.add_css_class("chip");
            add.add_css_class("addchip");
            add.set_can_focus(false);
            add.set_tooltip_text(Some("Add a reaction"));
            let at = w.at.clone();
            add.connect_clicked(move |_| send(&at.borrow(), "react"));
            w.chips.append(&add);
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

    fn unbind(&mut self, w: &mut Widgets, _root: &mut gtk::Overlay) {
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
