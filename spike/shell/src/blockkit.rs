//! Block Kit as widgets.
//!
//! The terminal drew these as text with a gutter. Here a header is a heading,
//! fields are a grid, a button is a button that opens its URL — which is the
//! single clearest argument for the pivot, and the reason spike criterion A5
//! exists.

use crate::markup;
use gtk::prelude::*;
use slk_core::blocks::{Accessory, Block, Button};

fn label(markup: &str, dim: bool) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_use_markup(true);
    let dimmed;
    l.set_markup(if dim {
        dimmed = format!(
            "<span foreground=\"{}\">{markup}</span>",
            markup::PALETTE.dim
        );
        &dimmed
    } else {
        markup
    });
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_xalign(0.0);
    l.set_halign(gtk::Align::Fill);
    l.set_hexpand(true);
    l.set_selectable(true);
    // A wrapping label in a horizontal box reports a minimum height (at its
    // minimum width) above its natural height (at its natural width), and
    // GTK 4.20 warns about it on every row. Bounding the natural width keeps
    // the two consistent.
    l.set_max_width_chars(70);
    l
}

fn button(b: &Button) -> gtk::Button {
    let w = gtk::Button::with_label(&b.label);
    match b.style.as_deref() {
        Some("primary") => w.add_css_class("suggested-action"),
        Some("danger") => w.add_css_class("destructive-action"),
        _ => {}
    }
    match &b.url {
        Some(url) => {
            let url = url.clone();
            w.connect_clicked(move |btn| {
                // The desktop's own opener, through the portal when sandboxed.
                let launcher = gtk::UriLauncher::new(&url);
                let window = btn.root().and_downcast::<gtk::Window>();
                launcher.launch(window.as_ref(), None::<&gtk::gio::Cancellable>, |r| {
                    if let Err(e) = r {
                        tracing::warn!("open {e}");
                    }
                });
            });
        }
        // A button that posts back to the app is a non-goal: it is drawn,
        // says so, and does nothing.
        None => {
            w.set_sensitive(false);
            w.set_tooltip_text(Some("interactive buttons are not supported"));
        }
    }
    w
}

pub fn widgets(blocks: &[Block], ctx: &markup::Ctx) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.add_css_class("blockkit");
    for b in blocks {
        match b {
            Block::Header(t) => {
                let l = label(
                    &format!("<big><b>{}</b></big>", gtk::glib::markup_escape_text(t)),
                    false,
                );
                root.append(&l);
            }
            Block::Section {
                text,
                fields,
                accessory,
            } => {
                // Stacked, not side by side. A horizontal box holding a
                // wrapping label reports a minimum height (at minimum width)
                // above its natural height, and GTK 4.20 warns on every bind;
                // measured, that was 1 700 warnings in one scroll. Vertical
                // boxes measure cleanly, and the accessory below the text is
                // how Slack lays it out on a narrow screen anyway.
                let col = gtk::Box::new(gtk::Orientation::Vertical, 6);
                if let Some(d) = text {
                    col.append(&label(&markup::doc(d, ctx), false));
                }
                if !fields.is_empty() {
                    let grid = gtk::Grid::new();
                    grid.set_column_spacing(18);
                    grid.set_row_spacing(4);
                    for (i, f) in fields.iter().enumerate() {
                        let l = label(&markup::doc(f, ctx), false);
                        l.set_hexpand(true);
                        grid.attach(&l, (i % 2) as i32, (i / 2) as i32, 1, 1);
                    }
                    col.append(&grid);
                }
                match accessory {
                    Some(Accessory::Button(b)) => {
                        let w = button(b);
                        w.set_halign(gtk::Align::Start);
                        col.append(&w);
                    }
                    Some(Accessory::Image { alt, .. }) => col.append(&label(
                        &format!("[image: {}]", gtk::glib::markup_escape_text(alt)),
                        true,
                    )),
                    Some(Accessory::Other(t)) => col.append(&label(
                        &format!("[{}]", gtk::glib::markup_escape_text(t)),
                        true,
                    )),
                    None => {}
                }
                root.append(&col);
            }
            Block::Context(items) => {
                let joined = items
                    .iter()
                    .map(|d| markup::doc(d, ctx).replace('\n', " "))
                    .collect::<Vec<_>>()
                    .join(" · ");
                root.append(&label(&format!("<small>{joined}</small>"), true));
            }
            Block::Divider => root.append(&gtk::Separator::new(gtk::Orientation::Horizontal)),
            Block::Image { alt, title, .. } => {
                if let Some(t) = title {
                    root.append(&label(
                        &format!("<b>{}</b>", gtk::glib::markup_escape_text(t)),
                        false,
                    ));
                }
                root.append(&label(
                    &format!("[image: {}]", gtk::glib::markup_escape_text(alt)),
                    true,
                ));
            }
            Block::Actions(bs) => {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                for b in bs {
                    row.append(&button(b));
                }
                root.append(&row);
            }
            Block::RichText(d) => root.append(&label(&markup::doc(d, ctx), false)),
            Block::Unsupported(t) => root.append(&label(
                &format!(
                    "<i>[unsupported block: {}]</i>",
                    gtk::glib::markup_escape_text(t)
                ),
                true,
            )),
        }
    }
    root
}
