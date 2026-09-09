//! The window: sidebar, conversation, composer — fed by the engine.
//!
//! `App` owns the component model and the channel ends, and nothing else.
//! Every `Event` arrives as an input message; every action the user takes
//! becomes a `Command`. The engine is on other threads and the model never
//! blocks on it.

use crate::bench::{self, Options};
use crate::row::Row;
use gtk::prelude::*;
use relm4::prelude::*;
use relm4::typed_view::list::TypedListView;
use slk_core::{ChannelId, Names, UserId};
use slk_sync::{Command, Event, SidebarEntry};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use tokio::sync::mpsc;

/// Name lookups for the renderer.
#[derive(Default)]
pub struct Directory {
    pub users: HashMap<String, String>,
    pub channels: HashMap<String, String>,
}

impl Names for Directory {
    fn user(&self, id: &str) -> Option<&str> {
        self.users.get(id).map(String::as_str)
    }
    fn channel(&self, id: &str) -> Option<&str> {
        self.channels.get(id).map(String::as_str)
    }
}

/// What every row needs and the model owns: names, the texture cache, the
/// pictures waiting for a texture, and a way to ask for one.
pub struct Shared {
    pub names: RefCell<Directory>,
    pub self_id: UserId,
    pub textures: RefCell<HashMap<String, gtk::gdk::Texture>>,
    pub pending: RefCell<HashMap<String, Vec<gtk::Picture>>>,
    sender: relm4::Sender<Msg>,
}

impl Shared {
    pub fn need_image(&self, file_id: &str, url: &str) {
        let _ = self.sender.send(Msg::NeedImage {
            file_id: file_id.to_string(),
            url: url.to_string(),
        });
    }
}

pub struct Init {
    pub commands: mpsc::Sender<Command>,
    pub events: Option<mpsc::Receiver<Event>>,
    pub runtime: tokio::runtime::Handle,
    pub media_dir: std::path::PathBuf,
    pub options: Options,
}

#[derive(Debug)]
pub enum Msg {
    Engine(Event),
    Open(usize),
    Send(String),
    NeedImage { file_id: String, url: String },
    Mapped,
    BenchScrollDone,
    IdleDone,
}

pub struct App {
    commands: mpsc::Sender<Command>,
    runtime: tokio::runtime::Handle,
    media_dir: std::path::PathBuf,
    options: Options,
    shared: Rc<Shared>,
    convs: Vec<SidebarEntry>,
    open: Option<ChannelId>,
    list: TypedListView<Row, gtk::NoSelection>,
    // Made before the view and referenced into it, because `update` needs a
    // handle to both and relm4 hands widgets to the view, not the model.
    sidebar: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    requested: HashSet<String>,
    status: String,
    title: String,
    loaded_once: bool,
    bench_started: bool,
    sent_at: Option<std::time::Instant>,
}

impl App {
    fn send(&self, cmd: Command) {
        let tx = self.commands.clone();
        // Fire and forget on the runtime; the main thread does not wait.
        self.runtime.spawn(async move {
            let _ = tx.send(cmd).await;
        });
    }

    fn open_channel(&mut self, id: ChannelId) {
        if self.open.as_ref() == Some(&id) {
            return;
        }
        self.title = self
            .convs
            .iter()
            .find(|c| c.id == id)
            .map(|c| {
                if c.is_dm() {
                    c.name.clone()
                } else {
                    format!("#{}", c.name)
                }
            })
            .unwrap_or_default();
        self.open = Some(id.clone());
        self.list.clear();
        self.send(Command::Open(id));
    }

    /// Index of the first row matching `f`. relm4 has `find`, behind a
    /// `gnome_43` feature that drags libadwaita in; a linear pass over a
    /// list this size is a few microseconds and needs nothing.
    fn position(&self, mut f: impl FnMut(&Row) -> bool) -> Option<u32> {
        self.list
            .iter()
            .position(|item| f(&item.borrow()))
            .map(|i| i as u32)
    }

    fn scroll_to_end(&self) {
        let n = self.list.len();
        if n > 0 {
            self.list
                .view
                .scroll_to(n - 1, gtk::ListScrollFlags::NONE, None);
        }
    }
}

#[relm4::component(pub)]
impl SimpleComponent for App {
    type Init = Init;
    type Input = Msg;
    type Output = ();

    view! {
        #[root]
        window = gtk::Window {
            set_title: Some("slack-light — spike A"),
            set_default_size: (1100, 720),
            connect_map[sender] => move |w| {
                bench::record_frames(w);
                sender.input(Msg::Mapped);
            },

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                gtk::Paned {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_position: 240,
                    set_vexpand: true,
                    set_shrink_start_child: false,

                    #[wrap(Some)]
                    set_start_child = &gtk::ScrolledWindow {
                        set_hscrollbar_policy: gtk::PolicyType::Never,
                        #[local_ref]
                        sidebar -> gtk::ListBox {
                            add_css_class: "navigation-sidebar",
                            connect_row_selected[sender] => move |_, row| {
                                if let Some(r) = row {
                                    sender.input(Msg::Open(r.index() as usize));
                                }
                            },
                        },
                    },

                    #[wrap(Some)]
                    set_end_child = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,

                        gtk::Label {
                            #[watch]
                            set_markup: &format!("<b>{}</b>", gtk::glib::markup_escape_text(&model.title)),
                            set_xalign: 0.0,
                            set_margin_all: 8,
                        },
                        gtk::Separator {},

                        #[local_ref]
                        scroller -> gtk::ScrolledWindow {
                            set_vexpand: true,
                            set_hscrollbar_policy: gtk::PolicyType::Never,
                            #[local_ref]
                            list_view -> gtk::ListView {
                                add_css_class: "conversation",
                            }
                        },

                        gtk::Entry {
                            set_margin_all: 8,
                            set_placeholder_text: Some("Message…  (Enter sends)"),
                            connect_activate[sender] => move |e| {
                                let text = e.text().to_string();
                                if !text.trim().is_empty() {
                                    sender.input(Msg::Send(text));
                                    e.set_text("");
                                }
                            },
                        },
                    },
                },

                gtk::Label {
                    #[watch]
                    set_label: &model.status,
                    set_xalign: 0.0,
                    set_margin_start: 8,
                    set_margin_end: 8,
                    set_margin_top: 2,
                    set_margin_bottom: 2,
                    add_css_class: "dim-label",
                },
            }
        }
    }

    fn init(init: Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let Init {
            commands,
            events,
            runtime,
            media_dir,
            options,
        } = init;

        // Events from the engine become input messages. The forwarder runs on
        // the runtime; `Sender::send` is the only thing that crosses threads.
        if let Some(mut rx) = events {
            let tx = sender.input_sender().clone();
            runtime.spawn(async move {
                while let Some(ev) = rx.recv().await {
                    if tx.send(Msg::Engine(ev)).is_err() {
                        return;
                    }
                }
            });
        }

        let shared = Rc::new(Shared {
            names: RefCell::new(Directory::default()),
            self_id: UserId::new("U0SELF"),
            textures: RefCell::new(HashMap::new()),
            pending: RefCell::new(HashMap::new()),
            sender: sender.input_sender().clone(),
        });

        let model = App {
            commands,
            runtime,
            media_dir,
            options,
            shared,
            convs: Vec::new(),
            open: None,
            list: TypedListView::new(),
            sidebar: gtk::ListBox::new(),
            scroller: gtk::ScrolledWindow::new(),
            requested: HashSet::new(),
            status: "starting".into(),
            title: String::new(),
            loaded_once: false,
            bench_started: false,
            sent_at: None,
        };
        let list_view = &model.list.view;
        let sidebar = &model.sidebar;
        let scroller = &model.scroller;
        let widgets = view_output!();

        relm4::set_global_css(
            ".conversation > row { border-bottom: 1px solid alpha(currentColor, 0.08); } \
             .conversation { background: transparent; } \
             .blockkit { padding: 6px 8px; border-left: 3px solid alpha(currentColor, 0.25); }",
        );

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>) {
        match msg {
            Msg::Mapped => {
                bench::report("first_map_ms", bench::since_start().as_millis());
                bench::report_memory("map");
            }
            Msg::Open(i) => {
                if let Some(c) = self.convs.get(i) {
                    let id = c.id.clone();
                    self.open_channel(id);
                }
            }
            Msg::Send(text) => {
                if let Some(ch) = self.open.clone() {
                    self.sent_at = Some(std::time::Instant::now());
                    self.send(Command::Send {
                        channel: ch,
                        thread: None,
                        text,
                        local_id: format!("spike-{}", bench::since_start().as_nanos()),
                        broadcast: false,
                    });
                }
            }
            Msg::NeedImage { file_id, url } => {
                if self.requested.insert(file_id.clone()) {
                    self.send(Command::FetchImage {
                        url,
                        file_id,
                        dir: self.media_dir.clone(),
                    });
                }
            }
            Msg::Engine(ev) => self.apply(ev, &sender),
            Msg::BenchScrollDone => {
                if self.options.idle_secs > 0 {
                    let cpu0 = bench::cpu_ns();
                    let secs = self.options.idle_secs;
                    let s = sender.input_sender().clone();
                    bench::reset_frames();
                    gtk::glib::timeout_add_local_once(
                        std::time::Duration::from_secs(secs),
                        move || {
                            let used = (bench::cpu_ns() - cpu0) as f64 / 1e9;
                            bench::report("idle_secs", secs);
                            bench::report(
                                "idle_cpu_pct",
                                format!("{:.2}", 100.0 * used / secs as f64),
                            );
                            bench::report_memory("idle");
                            bench::frame_summary("idle");
                            let _ = s.send(Msg::IdleDone);
                        },
                    );
                } else {
                    relm4::main_application().quit();
                }
            }
            Msg::IdleDone => relm4::main_application().quit(),
        }
    }
}

impl App {
    fn apply(&mut self, ev: Event, sender: &ComponentSender<Self>) {
        match ev {
            Event::Ready { .. } => self.status = "ready".into(),
            Event::Connected => self.status = "✓ connected".into(),
            Event::Disconnected(why) => self.status = format!("disconnected: {why}"),
            Event::Notice(t) => self.status = t,
            Event::Users(users) => {
                let mut d = self.shared.names.borrow_mut();
                for u in users {
                    d.users.insert(u.id.as_str().to_string(), u.label);
                }
            }
            Event::Conversations(convs) => {
                {
                    let mut d = self.shared.names.borrow_mut();
                    for c in &convs {
                        d.channels.insert(c.id.as_str().to_string(), c.name.clone());
                    }
                }
                self.convs = convs;
                self.rebuild_sidebar();
                if self.open.is_none() {
                    // Land where there is something to read, as the client did.
                    let pick = self
                        .convs
                        .iter()
                        .position(|c| c.mentions > 0 && !c.is_muted)
                        .or_else(|| self.convs.iter().position(|c| c.unread > 0))
                        .unwrap_or(0);
                    if let Some(c) = self.convs.get(pick) {
                        let id = c.id.clone();
                        self.open_channel(id);
                    }
                }
            }
            Event::Messages {
                channel,
                messages,
                append_older,
            } => {
                if self.open.as_ref() != Some(&channel) {
                    return;
                }
                let n = messages.len();
                if !append_older {
                    self.list.clear();
                }
                let shared = self.shared.clone();
                self.list
                    .extend_from_iter(messages.into_iter().map(|m| Row {
                        msg: m,
                        shared: shared.clone(),
                    }));
                self.scroll_to_end();
                self.status = format!("{n} messages");
                if !self.loaded_once {
                    self.loaded_once = true;
                    bench::report("first_messages_ms", bench::since_start().as_millis());
                    bench::report("rows", self.list.len());
                    bench::report_memory("loaded");
                    if let Some(row) = self.options.jump {
                        self.list.view.scroll_to(
                            row.min(self.list.len().saturating_sub(1)),
                            gtk::ListScrollFlags::NONE,
                            None,
                        );
                    }
                    if let Some(text) = self.options.send.clone() {
                        // Through `update`, exactly as the Entry's activate
                        // handler goes, so the timing is of the real path.
                        sender.input(Msg::Send(text));
                    }
                    if self.options.bench && !self.bench_started {
                        self.bench_started = true;
                        self.start_bench(sender);
                    }
                }
            }
            Event::Upserted {
                channel,
                message,
                replaces,
            } => {
                if self.open.as_ref() != Some(&channel) {
                    return;
                }
                if let Some(t0) = self.sent_at {
                    match (&message.delivery, &replaces) {
                        (slk_core::Delivery::Pending(_), None) => {
                            bench::report("send_optimistic_ms", t0.elapsed().as_millis())
                        }
                        (slk_core::Delivery::Confirmed, Some(_)) => {
                            bench::report("send_confirmed_ms", t0.elapsed().as_millis());
                            self.sent_at = None;
                        }
                        _ => {}
                    }
                }
                let target = replaces.unwrap_or_else(|| message.ts.clone());
                let existing = self.position(|r| r.msg.ts == target);
                let row = Row {
                    msg: *message,
                    shared: self.shared.clone(),
                };
                match existing {
                    Some(pos) => {
                        self.list.remove(pos);
                        self.list.insert(pos, row);
                    }
                    None => {
                        self.list.append(row);
                        self.scroll_to_end();
                    }
                }
            }
            Event::Deleted { channel, ts } => {
                if self.open.as_ref() == Some(&channel) {
                    if let Some(pos) = self.position(|r| r.msg.ts == ts) {
                        self.list.remove(pos);
                    }
                }
            }
            Event::ImageFetched { file_id, path } => {
                match gtk::gdk::Texture::from_filename(&path) {
                    Ok(t) => {
                        let waiting = self
                            .shared
                            .pending
                            .borrow_mut()
                            .remove(&file_id)
                            .unwrap_or_default();
                        for p in waiting {
                            p.set_paintable(Some(&t));
                        }
                        self.shared.textures.borrow_mut().insert(file_id, t);
                    }
                    Err(e) => tracing::warn!("texture {file_id}: {e}"),
                }
            }
            _ => {}
        }
    }

    fn rebuild_sidebar(&mut self) {
        // A plain ListBox: a dozen rows, not five thousand. Rebuilt
        // wholesale; the spike is not about the sidebar.
        while let Some(child) = self.sidebar.first_child() {
            self.sidebar.remove(&child);
        }
        for c in &self.convs {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.set_margin_start(8);
            row.set_margin_end(8);
            row.set_margin_top(4);
            row.set_margin_bottom(4);
            let name = gtk::Label::new(None);
            name.set_xalign(0.0);
            name.set_hexpand(true);
            let label = if c.is_dm() {
                c.name.clone()
            } else if c.is_private() {
                format!("🔒 {}", c.name)
            } else {
                format!("# {}", c.name)
            };
            if c.has_unread() {
                name.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(&label)));
            } else {
                name.set_label(&label);
            }
            row.append(&name);
            if c.mentions > 0 {
                let b = gtk::Label::new(None);
                b.set_markup(&format!(
                    "<span background=\"#d33\" foreground=\"white\"> {} </span>",
                    c.mentions
                ));
                row.append(&b);
            } else if c.unread > 0 {
                row.append(&gtk::Label::new(Some(&c.unread.to_string())));
            }
            self.sidebar.append(&row);
        }
    }

    fn start_bench(&mut self, sender: &ComponentSender<Self>) {
        let scroller = self.scroller.clone();
        let view = self.list.view.clone();
        let total = self.list.len();
        let s = sender.input_sender().clone();
        // A second for layout to settle; then twenty seconds of steady
        // scrolling upward from the end, recording every paint — a segment,
        // because the whole list at a readable pace is minutes and the
        // percentiles stop changing after a few hundred frames; then five
        // jumps across the list with a moment each, which is what realising
        // rows from cold costs.
        gtk::glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
            bench::reset_frames();
            let adj = scroller.vadjustment();
            let step = 40.0; // px per 16 ms tick ≈ 2 500 px/s
            let started = std::time::Instant::now();
            let s2 = s.clone();
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                let v = adj.value() - step;
                if v <= adj.lower() || started.elapsed().as_secs() >= 20 {
                    bench::report("scroll_px", (adj.upper() - adj.value()) as i64);
                    bench::frame_summary("scroll");
                    bench::report_memory("scroll");
                    // Jumps: far apart, so every one realises rows from cold.
                    bench::reset_frames();
                    let targets = [0u32, total / 2, total - 1, total / 4, (total * 3) / 4];
                    let mut i = 0;
                    let view = view.clone();
                    let s3 = s2.clone();
                    gtk::glib::timeout_add_local(
                        std::time::Duration::from_millis(400),
                        move || {
                            if i < targets.len() {
                                view.scroll_to(targets[i], gtk::ListScrollFlags::NONE, None);
                                i += 1;
                                gtk::glib::ControlFlow::Continue
                            } else {
                                bench::frame_summary("jumps");
                                bench::report_memory("jumps");
                                let _ = s3.send(Msg::BenchScrollDone);
                                gtk::glib::ControlFlow::Break
                            }
                        },
                    );
                    return gtk::glib::ControlFlow::Break;
                }
                adj.set_value(v);
                gtk::glib::ControlFlow::Continue
            });
        });
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let tx = self.commands.clone();
        self.runtime.spawn(async move {
            let _ = tx.send(Command::Shutdown).await;
        });
    }
}
