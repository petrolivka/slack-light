//! The window: sidebar, conversation, composer — fed by the engines.
//!
//! `App` owns the component model and the channel ends, and nothing else.
//! Every `Event` arrives as an input message tagged with its workspace;
//! every action the user takes becomes a `Command` to that workspace's
//! engine. The engines are on other threads and the model never blocks on
//! them.

use crate::bench::{self, Options};
use crate::row::Row;
use gtk::prelude::*;
use relm4::prelude::*;
use relm4::typed_view::list::TypedListView;
use slk_core::{ChannelId, Names, TeamId, UserId};
use slk_sync::{Command, Event, SidebarEntry};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use tokio::sync::mpsc;

/// Name lookups for the renderer. Ids are unique across workspaces, so one
/// directory serves all of them.
#[derive(Default)]
pub struct Directory {
    pub users: HashMap<String, String>,
    pub channels: HashMap<String, String>,
    /// The same, the other way round and lower-cased: rendering needs
    /// id → name, sending needs name → id, and completing needs to search
    /// names. One map cannot serve all three without a scan per mention.
    pub user_ids: HashMap<String, String>,
    pub channel_ids: HashMap<String, String>,
    /// Handle → id, for `@design-team`.
    pub groups: HashMap<String, String>,
}

impl Directory {
    pub fn add_user(&mut self, id: String, label: String) {
        self.user_ids.insert(label.to_lowercase(), id.clone());
        self.users.insert(id, label);
    }
    pub fn add_channel(&mut self, id: String, name: String) {
        self.channel_ids.insert(name.to_lowercase(), id.clone());
        self.channels.insert(id, name);
    }
}

impl Names for Directory {
    fn user(&self, id: &str) -> Option<&str> {
        self.users.get(id).map(String::as_str)
    }
    fn channel(&self, id: &str) -> Option<&str> {
        self.channels.get(id).map(String::as_str)
    }
}

impl slk_core::outgoing::Lookup for Directory {
    fn user_id(&self, name: &str) -> Option<String> {
        self.user_ids.get(&name.to_lowercase()).cloned()
    }
    fn channel_id(&self, name: &str) -> Option<String> {
        self.channel_ids.get(&name.to_lowercase()).cloned()
    }
    fn usergroup_id(&self, handle: &str) -> Option<String> {
        self.groups.get(&handle.to_lowercase()).cloned()
    }
}

/// What every row needs and the model owns: names, the palette, the texture
/// cache, the pictures waiting for a texture, and a way to ask for one.
pub struct Shared {
    pub names: RefCell<Directory>,
    /// Also how the sidebar and the rail send their own clicks: they are
    /// rebuilt from `&mut self`, where the component's sender is not in
    /// scope, and threading one through every call would be worse.
    pub sender: relm4::Sender<Msg>,
    pub pal: RefCell<slk_theme::Semantic>,
    pub self_id: RefCell<UserId>,
    pub textures: RefCell<HashMap<String, gtk::gdk::Texture>>,
    pub pending: RefCell<HashMap<String, Vec<gtk::Picture>>>,
}

thread_local! {
    /// Where a widget with no model behind it sends its actions.
    ///
    /// A pooled list row is built once and rebound thousands of times; its
    /// buttons outlive every `Row` they were ever shown for, so they cannot
    /// hold a `ComponentSender`. Everything here runs on the GTK main thread
    /// and there is exactly one window, which is what makes a thread-local
    /// the honest shape rather than a shortcut.
    static ROW_SENDER: RefCell<Option<relm4::Sender<Msg>>> = const { RefCell::new(None) };
}

/// Follow a link from inside a message. A Slack permalink is a jump, not a
/// trip to the browser, and only the window knows which.
pub fn link_clicked(url: &str) -> bool {
    ROW_SENDER.with_borrow(|s| match s {
        Some(s) => s.send(Msg::Link(url.to_string())).is_ok(),
        None => false,
    })
}

/// Run an action against one message, from a widget. See `ROW_SENDER`.
pub fn row_action(at: &str, act: &str) {
    ROW_SENDER.with_borrow(|s| {
        if let Some(s) = s {
            let _ = s.send(Msg::RowAction {
                at: at.to_string(),
                act: act.to_string(),
            });
        }
    });
}

impl Shared {
    pub fn need_image(&self, file_id: &str, url: &str) {
        let _ = self.sender.send(Msg::NeedImage {
            file_id: file_id.to_string(),
            url: url.to_string(),
        });
    }
}

/// One connected workspace, as the binary hands it over.
pub struct Workspace {
    pub team: TeamId,
    pub name: String,
    pub commands: mpsc::Sender<Command>,
}

pub struct Init {
    pub workspaces: Vec<Workspace>,
    /// Every workspace's events, folded into one stream and tagged.
    pub events: Option<mpsc::Receiver<(TeamId, Event)>>,
    pub runtime: tokio::runtime::Handle,
    pub media_dir: std::path::PathBuf,
    pub config: slk_config::Config,
    pub theme: slk_theme::Choice,
    pub user_css: Option<std::path::PathBuf>,
    pub read_only: bool,
    pub options: Options,
}

/// Which list the message cursor is in. Every message action reads it, so
/// "react" in a thread cannot land on the conversation behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Conv,
    Thread,
}

#[derive(Debug)]
pub enum Msg {
    ThemeChanged,
    /// A keyboard action, by its `slk-config` name.
    Action(String),
    /// The same, aimed at one message: what the row's own buttons send.
    /// `act` is either an action name or `react:<shortcode>`.
    RowAction {
        at: String,
        act: String,
    },
    /// A shortcode chosen in the picker.
    Picked(String),
    /// The composer's text changed; re-read what is being completed.
    ComposerChanged,
    /// The conversation's content changed size and has now been measured.
    Settled,
    /// The conversation moved, and where it is now.
    Scrolled {
        at_top: bool,
        at_bottom: bool,
    },
    /// A link inside a message was clicked.
    Link(String),
    /// Move within, or accept, the completion list.
    CompleteStep(i32),
    CompleteAccept,
    JumpChanged(String),
    JumpAccept,
    Engine(TeamId, Event),
    Open(usize),
    Send(String),
    SendThread(String),
    NeedImage {
        file_id: String,
        url: String,
    },
    /// The cursor moved into one of the two message lists.
    Focused(Pane),
    Mapped,
    StatusExpired(u32),
    ToggleSection(u8),
    BenchScrollDone,
    IdleDone,
}

/// The sidebar's groups, in the order the official client shows them.
const SECTIONS: [(u8, &str); 3] = [(0, "STARRED"), (1, "CHANNELS"), (2, "DIRECT MESSAGES")];

fn section_of(c: &SidebarEntry) -> u8 {
    if c.is_starred {
        0
    } else if c.is_dm() {
        2
    } else {
        1
    }
}

struct WorkspaceState {
    team: TeamId,
    name: String,
    commands: mpsc::Sender<Command>,
    convs: Vec<SidebarEntry>,
    connected: bool,
}

pub struct App {
    runtime: tokio::runtime::Handle,
    media_dir: std::path::PathBuf,
    options: Options,
    read_only: bool,
    shared: Rc<Shared>,
    workspaces: Vec<WorkspaceState>,
    current: usize,
    open: Option<ChannelId>,
    list: TypedListView<Row, gtk::SingleSelection>,
    /// The open thread's parent, and its replies.
    thread: Option<slk_core::Ts>,
    thread_list: TypedListView<Row, gtk::SingleSelection>,
    thread_pane: gtk::Box,
    thread_title: gtk::Label,
    thread_scroller: gtk::ScrolledWindow,
    thread_composer: gtk::TextView,
    follow: gtk::ToggleButton,
    broadcast: gtk::CheckButton,
    conv_paned: gtk::Paned,
    active: Pane,
    /// The message being edited, if the composer is holding one.
    editing: Option<slk_core::Ts>,
    /// Conversations visited, and where in them we are: back and forward.
    history: Vec<ChannelId>,
    history_at: usize,
    picker: Option<gtk::Window>,
    /// The completion popup: what it is offering, and where in the buffer
    /// the token being completed starts.
    complete: gtk::Popover,
    complete_list: gtk::ListBox,
    completing: Option<crate::logic::Completing>,
    /// Text to insert per offered row, alongside the row's own label.
    candidates: Vec<String>,
    /// Set while the client itself is rewriting the composer, and read
    /// **inside the signal handler**: `changed` is emitted synchronously but
    /// the input message it sends is handled later, by which time the flag
    /// is down again. Restoring a draft re-opened the completion popup that
    /// way, and the next Enter accepted a completion instead of sending.
    inserting: Rc<std::cell::Cell<bool>>,
    /// What was typed and not sent, per conversation. A draft lost by
    /// glancing at another channel is the thing people never forgive.
    drafts: HashMap<ChannelId, String>,
    /// Scrollback: whether a page is in flight, and whether the server has
    /// said there is nothing older left.
    loading_older: bool,
    at_beginning: bool,
    /// Whether the conversation is pinned to its newest message.
    ///
    /// Setting the adjustment once, when the messages arrive, does nothing:
    /// the rows have not been measured yet, so `upper` is still zero and
    /// "scroll to the bottom" clamps to the top. Every conversation longer
    /// than the window has opened at its *oldest* message since M1, which
    /// was invisible for as long as the demo fitted on one screen. The flag
    /// is what makes it stick — the bottom is re-taken every time the
    /// content grows, until the reader scrolls away from it.
    follow_bottom: bool,
    loading: gtk::Label,
    /// Slack's skin tone, applied to what the picker shows and to what is
    /// sent, so the two cannot disagree.
    skin: Option<u8>,
    // Made before the view and referenced into it, because `update` needs a
    // handle to each and relm4 hands widgets to the view, not the model.
    sidebar: gtk::ListBox,
    sidebar_box: gtk::Box,
    /// Row index in the sidebar to conversation index, because the section
    /// headings are rows too and the two stopped being the same thing.
    row_map: Vec<Option<usize>>,
    collapsed: HashSet<u8>,
    rail: gtk::Box,
    scroller: gtk::ScrolledWindow,
    composer: gtk::TextView,
    jump: gtk::Entry,
    jump_query: Rc<RefCell<String>>,
    bindings: Vec<crate::keys::Binding>,
    requested: HashSet<String>,
    theme: slk_theme::Applied,
    theme_choice: slk_theme::Choice,
    _theme_monitor: Option<gtk::gio::FileMonitor>,
    status: String,
    status_right: String,
    /// Under the composer: what Enter will do right now.
    hint: String,
    /// How many things have been said in the status bar. A notice's timer
    /// carries the number it was raised with, so an older one expiring
    /// cannot wipe a newer one — measured: "you can only edit your own
    /// messages" lasted a fraction of a second because a "pinned" from six
    /// seconds earlier chose that moment to clear itself.
    said: u32,
    title: String,
    topic: String,
    window_title: String,
    loaded_once: bool,
    bench_started: bool,
    sent_at: Option<std::time::Instant>,
}

impl App {
    fn ws(&self) -> Option<&WorkspaceState> {
        self.workspaces.get(self.current)
    }

    fn convs(&self) -> &[SidebarEntry] {
        self.ws().map(|w| w.convs.as_slice()).unwrap_or(&[])
    }

    /// Send to the current workspace's engine. Fire and forget on the
    /// runtime; the main thread does not wait.
    fn send(&self, cmd: Command) {
        if let Some(w) = self.ws() {
            let tx = w.commands.clone();
            self.runtime.spawn(async move {
                let _ = tx.send(cmd).await;
            });
        }
    }

    /// Open a conversation and remember it, so back and forward have
    /// somewhere to go. A conversation reached *by* back or forward calls
    /// `visit` instead, or the history would grow every time it was walked.
    fn open_channel(&mut self, id: ChannelId) {
        if self.open.as_ref() == Some(&id) {
            return;
        }
        if self.history.get(self.history_at) != Some(&id) {
            self.history
                .truncate(self.history_at + usize::from(!self.history.is_empty()));
            self.history.push(id.clone());
            self.history_at = self.history.len() - 1;
        }
        self.visit(id);
    }

    fn visit(&mut self, id: ChannelId) {
        if self.open.as_ref() == Some(&id) {
            return;
        }
        // An edit in flight belongs to the conversation being left.
        self.stash_draft();
        self.editing = None;
        self.close_completions();
        let found = self.convs().iter().find(|c| c.id == id).cloned();
        self.title = found
            .as_ref()
            .map(|c| {
                if c.is_dm() {
                    c.name.clone()
                } else if c.is_private() {
                    format!("🔒 {}", c.name)
                } else {
                    format!("# {}", c.name)
                }
            })
            .unwrap_or_default();
        self.topic = found
            .as_ref()
            .map(|c| {
                let mut bits = Vec::new();
                if let Some(n) = c.member_count.filter(|_| !c.is_dm()) {
                    bits.push(format!("{n} members"));
                }
                let note = if c.topic.is_empty() {
                    &c.purpose
                } else {
                    &c.topic
                };
                if !note.is_empty() {
                    bits.push(note.replace('\n', " "));
                }
                bits.join("  ·  ")
            })
            .unwrap_or_default();
        self.open = Some(id.clone());
        self.list.clear();
        // A thread belongs to the conversation it is in.
        if self.thread.is_some() {
            self.close_thread();
        }
        self.inserting.set(true);
        self.composer
            .buffer()
            .set_text(self.drafts.get(&id).map(String::as_str).unwrap_or(""));
        self.inserting.set(false);
        self.refresh_hint();
        self.send(Command::Open(id));
    }

    fn switch_to(&mut self, i: usize) {
        if i >= self.workspaces.len() || i == self.current {
            return;
        }
        self.stash_draft();
        self.current = i;
        self.open = None;
        self.list.clear();
        self.window_title = format!("slack-light — {}", self.workspaces[i].name);
        self.rebuild_sidebar();
        self.rebuild_rail();
        self.land();
    }

    /// Open the conversation worth opening in the current workspace.
    fn land(&mut self) {
        if self.open.is_some() {
            return;
        }
        let pick = crate::logic::landing(
            self.convs()
                .iter()
                .map(|c| (&c.is_muted, &c.unread, &c.mentions)),
        );
        if let Some(c) = self.convs().get(pick) {
            let id = c.id.clone();
            self.open_channel(id);
        }
    }

    /// Turn messages into rows, computing each one's grouping and
    /// separators from the one before it — including the row already at the
    /// end of the list, so a page of scrollback joins on correctly.
    fn make_rows(&self, msgs: Vec<slk_core::Message>, after_existing: bool) -> Vec<Row> {
        let last_read = self
            .convs()
            .iter()
            .find(|c| Some(&c.id) == self.open.as_ref())
            .and_then(|c| c.last_read.as_ref())
            .map(|t| t.secs());
        let mut prev: Option<(String, i64)> = if after_existing {
            self.list
                .len()
                .checked_sub(1)
                .and_then(|i| self.list.get(i))
                .map(|r| {
                    let m = &r.borrow().msg;
                    (m.author.id_str().to_string(), m.ts.secs())
                })
        } else {
            None
        };
        msgs.into_iter()
            .map(|m| {
                let meta = crate::logic::meta(
                    prev.as_ref().map(|(a, t)| (a.as_str(), *t)),
                    m.author.id_str(),
                    m.ts.secs(),
                    last_read,
                    day_label,
                );
                prev = Some((m.author.id_str().to_string(), m.ts.secs()));
                Row {
                    msg: m,
                    meta,
                    shared: self.shared.clone(),
                }
            })
            .collect()
    }

    /// One row, grouped against whatever is above it at `above`.
    ///
    /// `make_rows` computes each row's grouping from the one before it in the
    /// batch, which is right for a page and wrong for a replacement: an
    /// edited or echoed message is compared against the *end* of the list,
    /// which for the newest message is itself — same author, same second, so
    /// it groups under itself and loses its avatar, its name and its pin.
    fn row_at(&self, msg: slk_core::Message, above: Option<u32>) -> Row {
        let last_read = self
            .convs()
            .iter()
            .find(|c| Some(&c.id) == self.open.as_ref())
            .and_then(|c| c.last_read.as_ref())
            .map(|t| t.secs());
        let prev = above.and_then(|i| self.list.get(i)).map(|r| {
            let m = &r.borrow().msg;
            (m.author.id_str().to_string(), m.ts.secs())
        });
        let meta = crate::logic::meta(
            prev.as_ref().map(|(a, t)| (a.as_str(), *t)),
            msg.author.id_str(),
            msg.ts.secs(),
            last_read,
            day_label,
        );
        Row {
            msg,
            meta,
            shared: self.shared.clone(),
        }
    }

    /// A thread's rows. Grouped among themselves, with no day breaks and no
    /// unread line: a thread is read as one exchange, and a rule across a
    /// 360-pixel pane is noise rather than structure.
    fn thread_rows(&self, msgs: Vec<slk_core::Message>) -> Vec<Row> {
        let mut prev: Option<(String, i64)> = self
            .thread_list
            .len()
            .checked_sub(1)
            .and_then(|i| self.thread_list.get(i))
            .map(|r| {
                let m = &r.borrow().msg;
                (m.author.id_str().to_string(), m.ts.secs())
            });
        msgs.into_iter()
            .map(|m| {
                let mut meta = crate::logic::meta(
                    prev.as_ref().map(|(a, t)| (a.as_str(), *t)),
                    m.author.id_str(),
                    m.ts.secs(),
                    None,
                    day_label,
                );
                meta.day_break = None;
                prev = Some((m.author.id_str().to_string(), m.ts.secs()));
                Row {
                    msg: m,
                    meta,
                    shared: self.shared.clone(),
                }
            })
            .collect()
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

    /// Pin the conversation to its newest message.
    ///
    /// Through the adjustment rather than `ListView::scroll_to`: asking a
    /// list to scroll to an item that is already fully visible — which is
    /// every item, when the conversation is shorter than the window —
    /// leaves the list view retrying on a tick callback. Measured as a
    /// 60 Hz repaint on an idle window and 45 MB of churn. Moving the
    /// adjustment is a no-op when there is nothing to move.
    fn scroll_to_end(&mut self) {
        self.follow_bottom = true;
        self.pin_to_bottom();
    }

    /// Take the bottom, if we are not already on it. The guard matters: an
    /// unconditional `set_value` inside a `changed` handler is a repaint
    /// loop.
    fn pin_to_bottom(&self) {
        let adj = self.scroller.vadjustment();
        let want = (adj.upper() - adj.page_size()).max(0.0);
        if (adj.value() - want).abs() > 0.5 {
            adj.set_value(want);
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
            #[watch]
            set_title: Some(&model.window_title),
            set_default_size: (model.options.width, model.options.height),
            connect_map[sender] => move |w| {
                bench::record_frames(w);
                sender.input(Msg::Mapped);
            },

            gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,

                // The workspace rail, an activity bar: one square each, the
                // current one marked by an accent edge. Hidden when there is
                // only one workspace, because a switcher with one choice is
                // furniture.
                #[local_ref]
                rail -> gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    add_css_class: "rail",
                    #[watch]
                    set_visible: model.workspaces.len() > 1,
                },

                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_hexpand: true,

                    gtk::Paned {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_position: model.options.sidebar_width,
                        set_vexpand: true,
                        set_shrink_start_child: false,
                        // The extra width goes to the conversation. Without
                        // this the sidebar grows with the window, which on a
                        // tiling compositor means it grows constantly.
                        set_resize_start_child: false,
                        set_resize_end_child: true,

                        #[wrap(Some)]
                        #[local_ref]
                        set_start_child = sidebar_box -> gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            add_css_class: "sidebar",

                            #[local_ref]
                            jump -> gtk::Entry {
                                set_visible: false,
                                set_margin_all: 6,
                                set_placeholder_text: Some("jump to…"),
                                connect_changed[sender] => move |e| {
                                    sender.input(Msg::JumpChanged(e.text().to_string()));
                                },
                                connect_activate[sender] => move |_| sender.input(Msg::JumpAccept),
                            },

                            gtk::ScrolledWindow {
                                set_hscrollbar_policy: gtk::PolicyType::Never,
                                set_vexpand: true,
                                #[local_ref]
                                sidebar -> gtk::ListBox {
                                    connect_row_selected[sender] => move |_, row| {
                                        if let Some(r) = row {
                                            sender.input(Msg::Open(r.index() as usize));
                                        }
                                    },
                                },
                            },
                        },

                        // Conversation on the left, the open thread on the
                        // right — the official client's shape, and the one
                        // that keeps the thread readable next to what it is
                        // replying to rather than instead of it.
                        #[wrap(Some)]
                        #[local_ref]
                        set_end_child = conv_paned -> gtk::Paned {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_shrink_end_child: false,
                        set_resize_end_child: false,

                        #[wrap(Some)]
                        set_start_child = &gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_spacing: 10,
                                add_css_class: "header",
                                gtk::Label {
                                    #[watch]
                                    set_label: &model.title,
                                    add_css_class: "title",
                                },
                                gtk::Label {
                                    #[watch]
                                    set_label: &model.topic,
                                    #[watch]
                                    set_visible: !model.topic.is_empty(),
                                    set_ellipsize: gtk::pango::EllipsizeMode::End,
                                    set_xalign: 0.0,
                                    set_hexpand: true,
                                    add_css_class: "topic",
                                },
                            },
                            gtk::Box { add_css_class: "hairline" },

                            // Scrollback's own line. A conversation that
                            // silently stops at fifty messages reads as a
                            // conversation that only has fifty.
                            #[local_ref]
                            loading -> gtk::Label {
                                set_visible: false,
                                add_css_class: "loading",
                            },

                            #[local_ref]
                            scroller -> gtk::ScrolledWindow {
                                set_vexpand: true,
                                set_hscrollbar_policy: gtk::PolicyType::Never,
                                // Always, not Automatic: a conversation whose
                                // height lands within a few pixels of the
                                // viewport makes the scrollbar appear, which
                                // narrows the rows, which rewraps a label,
                                // which shortens the content, which hides the
                                // scrollbar. Measured: a 60 Hz repaint on an
                                // idle window, non-deterministic because it
                                // depends where the list settles. A scrollbar
                                // that is always there cannot toggle.
                                set_vscrollbar_policy: gtk::PolicyType::Always,
                                #[local_ref]
                                list_view -> gtk::ListView {
                                    add_css_class: "conversation",
                                }
                            },

                            // A panel, not a form field: several lines, its
                            // own border, the hint underneath.
                            gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                add_css_class: "composer",

                                gtk::ScrolledWindow {
                                    // Fixed bounds, not `propagate-natural-height`:
                                    // a ScrolledWindow that asks its TextView how
                                    // tall it wants to be, inside a box that then
                                    // changes the width, never settles — measured
                                    // as a 60 Hz repaint on an idle window.
                                    set_min_content_height: 34,
                                    set_max_content_height: 160,
                                    set_hscrollbar_policy: gtk::PolicyType::Never,
                                    #[local_ref]
                                    composer -> gtk::TextView {
                                        set_wrap_mode: gtk::WrapMode::WordChar,
                                        set_top_margin: 5,
                                        set_bottom_margin: 5,
                                        set_left_margin: 9,
                                        set_right_margin: 9,
                                        set_accepts_tab: false,
                                        #[watch]
                                        set_editable: !model.read_only,
                                    },
                                },
                                gtk::Label {
                                    set_xalign: 0.0,
                                    add_css_class: "hint",
                                    #[watch]
                                    set_label: &model.hint,
                                },
                            },
                        },

                        // The thread pane. Hidden rather than absent: a pane
                        // that is built when it is first needed opens a frame
                        // late, and this one opens on every reply.
                        #[wrap(Some)]
                        #[local_ref]
                        set_end_child = thread_pane -> gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_visible: false,
                            set_size_request: (360, -1),
                            add_css_class: "threadpane",

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_spacing: 8,
                                add_css_class: "header",
                                #[local_ref]
                                thread_title -> gtk::Label {
                                    set_xalign: 0.0,
                                    set_hexpand: true,
                                    set_ellipsize: gtk::pango::EllipsizeMode::End,
                                    add_css_class: "title",
                                },
                                #[local_ref]
                                follow -> gtk::ToggleButton {
                                    set_can_focus: false,
                                    add_css_class: "flat",
                                    set_tooltip_text: Some("Follow this thread"),
                                    connect_toggled[sender] => move |_| {
                                        sender.input(Msg::Action("follow_thread".into()));
                                    },
                                },
                                gtk::Button {
                                    set_label: "✕",
                                    set_can_focus: false,
                                    add_css_class: "flat",
                                    set_tooltip_text: Some("Close the thread"),
                                    connect_clicked[sender] => move |_| {
                                        sender.input(Msg::Action("close_thread".into()));
                                    },
                                },
                            },
                            gtk::Box { add_css_class: "hairline" },

                            #[local_ref]
                            thread_scroller -> gtk::ScrolledWindow {
                                set_vexpand: true,
                                set_hscrollbar_policy: gtk::PolicyType::Never,
                                set_vscrollbar_policy: gtk::PolicyType::Always,
                                #[local_ref]
                                thread_view -> gtk::ListView {
                                    add_css_class: "conversation",
                                }
                            },

                            gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                add_css_class: "composer",

                                gtk::ScrolledWindow {
                                    set_min_content_height: 34,
                                    set_max_content_height: 120,
                                    set_hscrollbar_policy: gtk::PolicyType::Never,
                                    #[local_ref]
                                    thread_composer -> gtk::TextView {
                                        set_wrap_mode: gtk::WrapMode::WordChar,
                                        set_top_margin: 5,
                                        set_bottom_margin: 5,
                                        set_left_margin: 9,
                                        set_right_margin: 9,
                                        set_accepts_tab: false,
                                        #[watch]
                                        set_editable: !model.read_only,
                                    },
                                },
                                #[local_ref]
                                broadcast -> gtk::CheckButton {
                                    set_label: Some("Also send to the conversation"),
                                    add_css_class: "hint",
                                },
                            },
                        },
                        },
                    },

                    // The status bar, in the shape an editor puts it.
                    gtk::Box {
                        set_orientation: gtk::Orientation::Horizontal,
                        add_css_class: "status",
                        gtk::Label {
                            #[watch]
                            set_label: &model.status,
                            set_xalign: 0.0,
                            set_hexpand: true,
                            set_ellipsize: gtk::pango::EllipsizeMode::End,
                        },
                        gtk::Label {
                            #[watch]
                            set_label: &model.status_right,
                            set_xalign: 1.0,
                        },
                    },
                },
            }
        }
    }

    fn init(init: Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let Init {
            workspaces,
            events,
            runtime,
            media_dir,
            config,
            theme: theme_choice,
            user_css,
            read_only,
            options,
        } = init;

        let (palette, source) = slk_theme::load(&theme_choice);
        bench::report("theme_source", format!("{source:?}"));
        let theme = slk_theme::Applied::new(&palette, user_css.as_deref());
        let monitor = {
            let s = sender.input_sender().clone();
            slk_theme::watch(move || {
                let _ = s.send(Msg::ThemeChanged);
            })
        };

        // Events from the engines become input messages. The forwarder runs
        // on the runtime; `Sender::send` is the only thing that crosses.
        if let Some(mut rx) = events {
            let tx = sender.input_sender().clone();
            runtime.spawn(async move {
                while let Some((team, ev)) = rx.recv().await {
                    if tx.send(Msg::Engine(team, ev)).is_err() {
                        return;
                    }
                }
            });
        }

        let shared = Rc::new(Shared {
            names: RefCell::new(Directory::default()),
            pal: RefCell::new(palette.semantic()),
            self_id: RefCell::new(UserId::new("")),
            textures: RefCell::new(HashMap::new()),
            pending: RefCell::new(HashMap::new()),
            sender: sender.input_sender().clone(),
        });

        let first_name = workspaces
            .first()
            .map(|w| w.name.clone())
            .unwrap_or_default();
        let model = App {
            runtime,
            media_dir,
            options: Options {
                width: config.window.width,
                height: config.window.height,
                sidebar_width: config.window.sidebar_width,
                ..options
            },
            read_only,
            shared,
            workspaces: workspaces
                .into_iter()
                .map(|w| WorkspaceState {
                    team: w.team,
                    name: w.name,
                    commands: w.commands,
                    convs: Vec::new(),
                    connected: false,
                })
                .collect(),
            current: 0,
            open: None,
            list: TypedListView::new(),
            thread: None,
            thread_list: TypedListView::new(),
            thread_pane: gtk::Box::new(gtk::Orientation::Vertical, 0),
            thread_title: gtk::Label::new(None),
            thread_scroller: gtk::ScrolledWindow::new(),
            thread_composer: gtk::TextView::new(),
            follow: gtk::ToggleButton::new(),
            broadcast: gtk::CheckButton::new(),
            conv_paned: gtk::Paned::new(gtk::Orientation::Horizontal),
            active: Pane::Conv,
            editing: None,
            history: Vec::new(),
            history_at: 0,
            picker: None,
            complete: gtk::Popover::new(),
            complete_list: gtk::ListBox::new(),
            completing: None,
            candidates: Vec::new(),
            inserting: Rc::new(std::cell::Cell::new(false)),
            drafts: HashMap::new(),
            loading_older: false,
            at_beginning: false,
            follow_bottom: true,
            loading: gtk::Label::new(None),
            skin: (config.emoji.skin_tone > 0).then_some(config.emoji.skin_tone),
            sidebar: gtk::ListBox::new(),
            sidebar_box: gtk::Box::new(gtk::Orientation::Vertical, 0),
            row_map: Vec::new(),
            collapsed: HashSet::new(),
            rail: gtk::Box::new(gtk::Orientation::Vertical, 0),
            scroller: gtk::ScrolledWindow::new(),
            composer: gtk::TextView::new(),
            jump: gtk::Entry::new(),
            jump_query: Rc::new(RefCell::new(String::new())),
            bindings: Vec::new(),
            requested: HashSet::new(),
            theme,
            theme_choice,
            _theme_monitor: monitor,
            status: "starting".into(),
            status_right: String::new(),
            said: 0,
            hint: if read_only {
                "read-only — nothing is sent".into()
            } else {
                "enter sends · shift+enter newline".into()
            },
            title: String::new(),
            topic: String::new(),
            window_title: if first_name.is_empty() {
                "slack-light".into()
            } else {
                format!("slack-light — {first_name}")
            },
            loaded_once: false,
            bench_started: false,
            sent_at: None,
        };
        let list_view = &model.list.view;
        let thread_view = &model.thread_list.view;
        let thread_pane = &model.thread_pane;
        let thread_title = &model.thread_title;
        let thread_scroller = &model.thread_scroller;
        let thread_composer = &model.thread_composer;
        let follow = &model.follow;
        let broadcast = &model.broadcast;
        let conv_paned = &model.conv_paned;
        let sidebar = &model.sidebar;
        let sidebar_box = &model.sidebar_box;
        let rail = &model.rail;
        let scroller = &model.scroller;
        let loading = &model.loading;
        let composer = &model.composer;
        let jump = &model.jump;
        let widgets = view_output!();

        // Under `--metrics`, log chords — and only chords. This is the
        // instrument that found the dead shift bindings: nothing else in the
        // stack says whether a key reached the application, and guessing
        // cost most of an afternoon. Plain keys are never logged, because
        // plain keys are the message the user is typing.
        {
            let k = gtk::EventControllerKey::new();
            k.set_propagation_phase(gtk::PropagationPhase::Capture);
            k.connect_key_pressed(|_, key, _, state| {
                use gtk::gdk::ModifierType as M;
                if state.intersects(M::CONTROL_MASK | M::ALT_MASK) {
                    bench::report(
                        "key",
                        format!("{} {state:?}", key.name().unwrap_or_default()),
                    );
                }
                gtk::glib::Propagation::Proceed
            });
            root.add_controller(k);
        }

        // Two things hang off the conversation's scrollbar.
        //
        // `changed` fires when the *content* changes size, which is when a
        // page of messages has finally been measured: that is the only
        // moment at which "scroll to the bottom" can mean anything.
        //
        // `value-changed` fires when the view moves, from the reader or from
        // us. Near the top it asks for the page before this one; near the
        // bottom it re-arms the follow, which is how scrolling back up and
        // then down again behaves the way people expect.
        {
            let adj = model.scroller.vadjustment();
            let s2 = sender.input_sender().clone();
            adj.connect_changed(move |_| {
                let _ = s2.send(Msg::Settled);
            });
            let s3 = sender.input_sender().clone();
            adj.connect_value_changed(move |a| {
                let _ = s3.send(Msg::Scrolled {
                    at_top: a.value() < 400.0 && a.upper() > a.page_size(),
                    at_bottom: a.upper() - (a.value() + a.page_size()) < 40.0,
                });
            });
        }

        // Where a pooled row's buttons send what they were clicked for.
        ROW_SENDER.with_borrow_mut(|slot| *slot = Some(sender.input_sender().clone()));

        // The completion popup. `autohide` off, because the composer has to
        // keep the keyboard: a popup that takes focus turns every completion
        // into a click to get back.
        {
            let pop = &model.complete;
            pop.set_autohide(false);
            pop.set_has_arrow(false);
            pop.set_position(gtk::PositionType::Top);
            pop.add_css_class("completions");
            let scroller = gtk::ScrolledWindow::new();
            scroller.set_max_content_height(220);
            scroller.set_propagate_natural_height(true);
            scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
            scroller.set_child(Some(&model.complete_list));
            pop.set_child(Some(&scroller));
            pop.set_parent(&model.composer);
            let s2 = sender.input_sender().clone();
            model.complete_list.connect_row_activated(move |list, row| {
                list.select_row(Some(row));
                let _ = s2.send(Msg::CompleteAccept);
            });
        }

        // Enter sends, shift+Enter is a newline. A TextView has no
        // `activate`, so each composer needs the key itself. While the
        // completion popup is up the same keys drive it instead — Enter that
        // sends a half-typed mention is worse than no completion at all.
        for (view, to_thread) in [(&model.composer, false), (&model.thread_composer, true)] {
            let k = gtk::EventControllerKey::new();
            let s2 = sender.input_sender().clone();
            let view2 = view.clone();
            let pop = model.complete.clone();
            k.connect_key_pressed(move |_, key, _, state| {
                use gtk::gdk::Key;
                let open = !to_thread && pop.is_visible();
                if open {
                    match key {
                        Key::Down | Key::Tab => {
                            let _ = s2.send(Msg::CompleteStep(1));
                            return gtk::glib::Propagation::Stop;
                        }
                        Key::Up | Key::ISO_Left_Tab => {
                            let _ = s2.send(Msg::CompleteStep(-1));
                            return gtk::glib::Propagation::Stop;
                        }
                        Key::Return | Key::KP_Enter => {
                            let _ = s2.send(Msg::CompleteAccept);
                            return gtk::glib::Propagation::Stop;
                        }
                        Key::Escape => {
                            let _ = s2.send(Msg::CompleteStep(0));
                            return gtk::glib::Propagation::Stop;
                        }
                        _ => {}
                    }
                }
                if key == Key::Return && !state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                    let buf = view2.buffer();
                    let (a, b) = buf.bounds();
                    let text = buf.text(&a, &b, false).to_string();
                    if !text.trim().is_empty() {
                        let _ = s2.send(if to_thread {
                            Msg::SendThread(text)
                        } else {
                            Msg::Send(text)
                        });
                        buf.set_text("");
                    }
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            });
            view.add_controller(k);
        }
        {
            let s2 = sender.input_sender().clone();
            let quiet = model.inserting.clone();
            model.composer.buffer().connect_changed(move |_| {
                if !quiet.get() {
                    let _ = s2.send(Msg::ComposerChanged);
                }
            });
        }

        // Nothing is selected until something is: an autoselecting list puts
        // the cursor on the oldest message every time a conversation loads,
        // and then alt-e edits whatever that happens to be.
        for (list, pane) in [
            (&model.list, Pane::Conv),
            (&model.thread_list, Pane::Thread),
        ] {
            list.selection_model.set_autoselect(false);
            list.selection_model.set_can_unselect(true);
            let s2 = sender.input_sender().clone();
            list.selection_model.connect_selected_item_notify(move |_| {
                let _ = s2.send(Msg::Focused(pane));
            });
        }

        // Keys: the preset's chords as accelerators on application actions.
        let mut model = model;
        model.bindings = crate::keys::install(
            &relm4::main_application(),
            &config.keymap.preset,
            sender.input_sender().clone(),
        );
        for b in &model.bindings {
            bench::report(
                "binding",
                format!("{}={} ({})", b.action.name(), b.accel, b.source),
            );
        }
        // The sidebar filters by the jump query; rows are (label, badge) boxes.
        {
            let q = model.jump_query.clone();
            model.sidebar.set_filter_func(move |row| {
                let q = q.borrow();
                if q.is_empty() {
                    return true;
                }
                // A heading is a row and matches nothing: while a query is
                // being typed the sidebar is a flat list of what matches.
                if !row.is_selectable() {
                    return false;
                }
                row.child()
                    .and_then(|b| b.first_child())
                    .and_downcast::<gtk::Label>()
                    .map(|l| l.text().to_lowercase().contains(q.to_lowercase().as_str()))
                    .unwrap_or(true)
            });
        }

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>) {
        crate::bench::count_update(&msg);
        match msg {
            Msg::ThemeChanged => {
                let (palette, source) = slk_theme::load(&self.theme_choice);
                self.theme.replace(&palette);
                *self.shared.pal.borrow_mut() = palette.semantic();
                // Rows carry their colours in their markup, so they have to
                // be bound again: detaching and reattaching the model does
                // exactly that for the realised ones and nothing for the rest.
                for list in [&self.list, &self.thread_list] {
                    let model = list.selection_model.clone();
                    list.view.set_model(None::<&gtk::SingleSelection>);
                    list.view.set_model(Some(&model));
                }
                self.rebuild_sidebar();
                bench::report("theme_source", format!("{source:?}"));
                self.say(format!("theme: {}", slk_theme::Applied::describe(&source)));
            }
            Msg::Action(name) => self.act(&name, &sender),
            Msg::JumpChanged(q) => {
                *self.jump_query.borrow_mut() = q;
                self.sidebar.invalidate_filter();
            }
            Msg::JumpAccept => {
                // The first row the filter left visible.
                let mut i = 0;
                while let Some(row) = self.sidebar.row_at_index(i) {
                    if row.is_child_visible() && row.is_selectable() {
                        self.sidebar.select_row(Some(&row));
                        break;
                    }
                    i += 1;
                }
                self.close_jump();
            }
            Msg::Mapped => {
                bench::report("first_map_ms", bench::since_start().as_millis());
                // Where the keyboard starts: writing. A window that opens
                // with focus nowhere is one where the first keystroke is lost.
                self.composer.grab_focus();
                self.rebuild_rail();
                bench::report_memory("map");
            }
            Msg::Open(row) => {
                if let Some(Some(i)) = self.row_map.get(row).copied() {
                    if let Some(c) = self.convs().get(i) {
                        let id = c.id.clone();
                        self.open_channel(id);
                    }
                }
            }
            Msg::StatusExpired(n) => {
                if n == self.said {
                    self.refresh_status()
                }
            }
            Msg::Focused(pane) => {
                // Clearing a list emits a selection change of its own, and
                // closing the thread clears one: without this, alt-w moved
                // the cursor *into* the pane it had just closed and every
                // message action afterwards said "no message selected".
                if pane == Pane::Conv || self.thread.is_some() {
                    self.active = pane;
                }
            }
            Msg::RowAction { at, act } => {
                // Point the cursor at the message the button belongs to, then
                // run exactly what the keyboard would have run.
                if let Some(pos) = self.position_in(Pane::Conv, &at) {
                    self.active = Pane::Conv;
                    self.list.selection_model.set_selected(pos);
                } else if let Some(pos) = self.position_in(Pane::Thread, &at) {
                    self.active = Pane::Thread;
                    self.thread_list.selection_model.set_selected(pos);
                }
                match act.strip_prefix("react:") {
                    Some(name) => self.react(name, &sender),
                    None => self.act(&act, &sender),
                }
            }
            Msg::ComposerChanged => self.refresh_completions(),
            Msg::Settled => {
                if self.follow_bottom {
                    self.pin_to_bottom();
                }
            }
            Msg::Scrolled { at_top, at_bottom } => {
                self.follow_bottom = at_bottom;
                if at_top {
                    self.load_older();
                }
            }
            Msg::Link(url) => self.follow_link(&url, &sender),
            Msg::CompleteStep(d) => {
                if d == 0 {
                    self.close_completions();
                } else {
                    let n = self.candidates.len() as i32;
                    if n > 0 {
                        let at = self
                            .complete_list
                            .selected_row()
                            .map(|r| r.index())
                            .unwrap_or(0);
                        let to = (at + d).rem_euclid(n);
                        if let Some(row) = self.complete_list.row_at_index(to) {
                            self.complete_list.select_row(Some(&row));
                            // Keep it in view: a selection scrolled out of
                            // the popup is a selection you cannot read.
                            row.grab_focus();
                            self.composer.grab_focus();
                        }
                    }
                }
            }
            Msg::CompleteAccept => self.accept_completion(),
            Msg::Picked(name) => {
                self.close_picker();
                self.react(&name, &sender);
            }
            Msg::SendThread(text) => {
                if self.read_only {
                    self.notice("read-only: nothing is sent".into(), &sender);
                    return;
                }
                if let (Some(ch), Some(parent)) = (self.open.clone(), self.thread.clone()) {
                    let text = self.wire(&text);
                    self.send(Command::Send {
                        channel: ch,
                        thread: Some(parent),
                        text,
                        local_id: format!("sl-{}", bench::since_start().as_nanos()),
                        broadcast: self.broadcast.is_active(),
                    });
                }
            }
            Msg::ToggleSection(sec) => {
                if !self.collapsed.remove(&sec) {
                    self.collapsed.insert(sec);
                }
                self.rebuild_sidebar();
            }
            Msg::Send(text) => {
                if self.read_only {
                    self.notice("read-only: nothing is sent".into(), &sender);
                    return;
                }
                let Some(ch) = self.open.clone() else { return };
                if let Some(ts) = self.editing.take() {
                    let text = self.wire(&text);
                    self.send(Command::Edit {
                        channel: ch,
                        ts,
                        text,
                    });
                    self.refresh_hint();
                    return;
                }
                self.sent_at = Some(std::time::Instant::now());
                self.drafts.remove(&ch);
                let text = self.wire(&text);
                self.send(Command::Send {
                    channel: ch,
                    thread: None,
                    text,
                    local_id: format!("sl-{}", bench::since_start().as_nanos()),
                    broadcast: false,
                });
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
            Msg::Engine(team, ev) => self.apply(team, ev, &sender),
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
                            bench::update_summary();
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
    fn apply(&mut self, team: TeamId, ev: Event, sender: &ComponentSender<Self>) {
        let Some(idx) = self.workspaces.iter().position(|w| w.team == team) else {
            return;
        };
        let is_current = idx == self.current;
        match ev {
            Event::Ready { self_id, .. } => {
                if is_current {
                    *self.shared.self_id.borrow_mut() = self_id;
                    self.refresh_status();
                }
            }
            Event::Connected => {
                self.workspaces[idx].connected = true;
                self.rebuild_rail();
                if is_current {
                    self.refresh_status();
                }
            }
            Event::Disconnected(why) => {
                self.workspaces[idx].connected = false;
                if is_current {
                    self.notice(format!("disconnected: {why}"), sender);
                }
            }
            Event::AuthLost => {
                self.say(format!(
                    "{}: session expired — run `slack-light auth add`",
                    self.workspaces[idx].name
                ));
            }
            Event::Notice(t) => {
                if is_current {
                    self.notice(t, sender);
                }
            }
            Event::RateLimited { seconds } => {
                self.notice(format!("Slack asked us to wait {seconds}s"), sender);
            }
            Event::Users(users) => {
                let mut d = self.shared.names.borrow_mut();
                for u in users {
                    d.add_user(u.id.as_str().to_string(), u.label);
                }
            }
            Event::UserGroups(groups) => {
                let mut d = self.shared.names.borrow_mut();
                for (id, handle) in groups {
                    d.groups.insert(handle.to_lowercase(), id);
                }
            }
            Event::Conversations(convs) => {
                {
                    let mut d = self.shared.names.borrow_mut();
                    for c in &convs {
                        d.add_channel(c.id.as_str().to_string(), c.name.clone());
                    }
                }
                self.workspaces[idx].convs = convs;
                if is_current {
                    self.rebuild_sidebar();
                    self.land();
                }
            }
            Event::Messages {
                channel,
                messages,
                append_older,
            } => {
                if !is_current || self.open.as_ref() != Some(&channel) {
                    return;
                }
                if append_older {
                    self.loading_older = false;
                    self.prepend_rows(messages);
                    self.loading.set_visible(self.at_beginning);
                    return;
                }
                self.list.clear();
                self.at_beginning = false;
                self.loading_older = false;
                self.loading.set_visible(false);
                let rows = self.make_rows(messages, false);
                self.list.extend_from_iter(rows);
                self.scroll_to_end();
                self.refresh_status();
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
                if !is_current || self.open.as_ref() != Some(&channel) {
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
                // A reply belongs in the thread pane; only a broadcast of
                // one also belongs in the conversation, and Slack marks that
                // by sending it without a `thread_ts` of its own.
                if let Some(parent) = self.thread.clone() {
                    let in_thread =
                        message.ts == parent || message.thread_ts.as_ref() == Some(&parent);
                    if in_thread {
                        match self.position_in(Pane::Thread, message.ts.as_str()) {
                            Some(pos) => self.replace_row(Pane::Thread, pos, (*message).clone()),
                            None => {
                                let rows = self.thread_rows(vec![(*message).clone()]);
                                self.thread_list.extend_from_iter(rows);
                                let adj = self.thread_scroller.vadjustment();
                                adj.set_value(adj.upper() - adj.page_size());
                            }
                        }
                        if message.ts != parent && message.thread_ts.is_some() {
                            return;
                        }
                    }
                }
                let rows: Vec<String> = self
                    .list
                    .iter()
                    .map(|r| r.borrow().msg.ts.as_str().to_string())
                    .collect();
                let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
                let place = crate::logic::placement(
                    &refs,
                    message.ts.as_str(),
                    replaces.as_ref().map(|t| t.as_str()),
                );
                let above = match place {
                    crate::logic::Placement::Replace(pos) => (pos as u32).checked_sub(1),
                    crate::logic::Placement::Append => self.list.len().checked_sub(1),
                };
                let row = self.row_at(*message, above);
                match place {
                    crate::logic::Placement::Replace(pos) => {
                        // A replaced row loses the selection with the widget
                        // it was on. Measured: saving a message put a notice
                        // on screen, the engine echoed the message back, and
                        // the next action said "no message selected" — with
                        // the cursor still visibly on the row.
                        let pos = pos as u32;
                        let had_cursor = self.list.selection_model.selected() == pos;
                        self.list.remove(pos);
                        self.list.insert(pos, row);
                        if had_cursor {
                            self.list.selection_model.set_selected(pos);
                            self.list
                                .view
                                .scroll_to(pos, gtk::ListScrollFlags::NONE, None);
                        }
                    }
                    crate::logic::Placement::Append => {
                        self.list.append(row);
                        self.scroll_to_end();
                    }
                }
            }
            Event::MessagesAround {
                channel,
                messages,
                focus,
            } => {
                if !is_current || self.open.as_ref() != Some(&channel) {
                    return;
                }
                self.list.clear();
                self.at_beginning = false;
                let rows = self.make_rows(messages, false);
                self.list.extend_from_iter(rows);
                // The point of jumping is to see the message in what was
                // said around it, so it is put on the cursor, not merely
                // scrolled to.
                self.active = Pane::Conv;
                if let Some(pos) = self.position(|r| r.msg.ts == focus) {
                    self.set_cursor(Some(pos as usize));
                } else {
                    self.notice("that message is no longer here".into(), sender);
                }
            }
            Event::Thread {
                channel,
                parent,
                messages,
            } => {
                if !is_current
                    || self.open.as_ref() != Some(&channel)
                    || self.thread.as_ref() != Some(&parent)
                {
                    return;
                }
                let head = messages.iter().find(|m| m.ts == parent);
                self.thread_title.set_label(&match head {
                    Some(m) => {
                        let names = self.shared.names.borrow();
                        let one = m.body.plain_with(&*names).replace('\n', " ");
                        format!("Thread · {}", one.chars().take(48).collect::<String>())
                    }
                    None => "Thread".into(),
                });
                if let Some(m) = head {
                    self.follow.set_active(m.subscribed);
                    self.refresh_follow_label();
                }
                self.thread_list.clear();
                // Every reply is its own author's, and grouping a thread the
                // way the conversation is grouped makes the parent read as
                // part of the reply above it.
                let rows = self.thread_rows(messages);
                self.thread_list.extend_from_iter(rows);
                let adj = self.thread_scroller.vadjustment();
                adj.set_value(adj.upper() - adj.page_size());
            }
            Event::Permalink { url, .. } => {
                self.to_clipboard(&url);
                self.notice("link copied".into(), sender);
            }
            Event::Deleted { channel, ts } => {
                if is_current && self.open.as_ref() == Some(&channel) {
                    if let Some(pos) = self.position(|r| r.msg.ts == ts) {
                        self.list.remove(pos);
                    }
                    if let Some(pos) = self.position_in(Pane::Thread, ts.as_str()) {
                        self.thread_list.remove(pos);
                    }
                    if self.thread.as_ref() == Some(&ts) {
                        self.close_thread();
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

    /// Put the sidebar's selection on a conversation, by its index.
    fn select_conv(&self, conv: usize) {
        if let Some(row_i) = self.row_map.iter().position(|m| *m == Some(conv)) {
            if let Some(row) = self.sidebar.row_at_index(row_i as i32) {
                self.sidebar.select_row(Some(&row));
            }
        }
    }

    fn close_jump(&mut self) {
        self.jump.set_text("");
        self.jump.set_visible(false);
        self.composer.grab_focus();
    }

    /// The left-hand end: what the client is, right now. A notice from the
    /// engine replaces it for a moment and then it comes back, because a
    /// status bar that keeps the last thing that happened is a log.
    fn refresh_status(&mut self) {
        let Some(w) = self.ws() else {
            self.status = "no workspace".into();
            return;
        };
        self.status = if w.connected {
            format!("✓ connected · {}", w.name)
        } else {
            format!("connecting · {}", w.name)
        };
    }

    /// A notice, shown for a few seconds and then replaced by the state.
    fn notice(&mut self, text: String, sender: &ComponentSender<Self>) {
        self.say(text);
        let s = sender.input_sender().clone();
        let n = self.said;
        gtk::glib::timeout_add_local_once(std::time::Duration::from_secs(6), move || {
            let _ = s.send(Msg::StatusExpired(n));
        });
    }

    /// Put something in the status bar and remember that it is the newest.
    fn say(&mut self, text: String) {
        self.said = self.said.wrapping_add(1);
        self.status = text;
    }

    /// The right-hand end of the status bar: what is waiting, and the two
    /// keys worth knowing.
    fn refresh_status_right(&mut self) {
        let mentions: u32 = self
            .workspaces
            .iter()
            .flat_map(|w| w.convs.iter())
            .filter(|c| !c.is_muted)
            .map(|c| c.mentions)
            .sum();
        self.status_right = if mentions > 0 {
            format!("↑ {mentions} mentions   ctrl-k jump   F1 keys")
        } else {
            "ctrl-k jump   F1 keys".to_string()
        };
    }

    /// One place where every action happens, whatever pressed it.
    fn act(&mut self, name: &str, sender: &ComponentSender<Self>) {
        match name {
            "jump_to" => {
                self.jump.set_visible(true);
                self.jump.grab_focus();
            }
            // Escape unwinds one layer at a time, innermost first. One key,
            // one meaning, and never "closed the thread when I meant to
            // dismiss the picker".
            "normal" => self.escape(),
            "next_conversation" | "prev_conversation" => {
                // Over what is on screen, in the order it is on screen: the
                // sidebar has headings now, so a row index is not a
                // conversation index, and a folded section has no rows at
                // all. Stepping over conversation indices walked into both.
                let shown: Vec<usize> = self.row_map.iter().filter_map(|m| *m).collect();
                let at = self
                    .sidebar
                    .selected_row()
                    .and_then(|r| self.row_map.get(r.index() as usize).copied().flatten())
                    .and_then(|c| shown.iter().position(|v| *v == c));
                if let Some(pos) = crate::logic::step(at, shown.len(), name == "next_conversation")
                {
                    self.select_conv(shown[pos]);
                }
                self.composer.grab_focus();
            }
            "next_unread" | "prev_unread" => {
                let shown: Vec<usize> = self
                    .row_map
                    .iter()
                    .filter_map(|m| *m)
                    .filter(|i| {
                        self.convs()
                            .get(*i)
                            .is_some_and(|c| !c.is_muted && (c.unread > 0 || c.mentions > 0))
                    })
                    .collect();
                if shown.is_empty() {
                    self.say("nothing unread".into());
                    return;
                }
                let here = self
                    .sidebar
                    .selected_row()
                    .and_then(|r| self.row_map.get(r.index() as usize).copied().flatten());
                let next = if name == "next_unread" {
                    shown.iter().find(|i| Some(**i) > here)
                } else {
                    shown.iter().rev().find(|i| Some(**i) < here)
                };
                if let Some(i) = next.or(shown.first()) {
                    self.select_conv(*i);
                }
            }
            "next_workspace" | "prev_workspace" => {
                let n = self.workspaces.len();
                if n > 1 {
                    let step = if name == "next_workspace" { 1 } else { n - 1 };
                    self.switch_to((self.current + step) % n);
                }
            }
            "clear_composer" => self.composer_view().buffer().set_text(""),
            "toggle_sidebar" => {
                let on = !self.sidebar_box.is_visible();
                self.sidebar_box.set_visible(on);
            }
            "back" | "forward" => self.go(name == "forward"),
            "help" | "palette" => self.show_shortcuts(),

            // Moving the cursor over messages. The list scrolls to follow it,
            // because a selection you cannot see is not a cursor.
            "cursor_up" => self.move_cursor(-1),
            "cursor_down" => self.move_cursor(1),
            "page_up" => self.move_cursor(-10),
            "page_down" => self.move_cursor(10),
            // The two ends move the scrollbar as well as the cursor.
            // `ListView::scroll_to` reaches a neighbouring row reliably and
            // the far end of five thousand not at all — measured: alt-Home
            // selected the oldest message and left the view where it was.
            "goto_oldest" => {
                self.set_cursor(Some(0));
                self.follow_bottom = false;
                self.scroller.vadjustment().set_value(0.0);
            }
            "goto_newest" => {
                let len = self.list_of(self.active).len();
                self.set_cursor(len.checked_sub(1).map(|i| i as usize));
                self.scroll_to_end();
            }

            "open_thread" => self.open_thread(sender),
            "close_thread" => self.close_thread(),
            "follow_thread" => {
                if let (Some(ch), Some(parent)) = (self.open.clone(), self.thread.clone()) {
                    self.send(Command::FollowThread {
                        channel: ch,
                        thread: parent,
                        on: self.follow.is_active(),
                    });
                    self.refresh_follow_label();
                }
            }
            "broadcast" => self.broadcast.set_active(!self.broadcast.is_active()),

            "react" => self.open_picker(sender),
            "mark_read" => {
                if let (Some(ch), Some(m)) = (self.open.clone(), self.newest()) {
                    self.send(Command::MarkRead(ch, m));
                    self.notice("marked read".into(), sender);
                }
            }
            "upload_file" => self.pick_file(sender),
            "search" | "search_local" | "threads" | "saved" | "mentions" | "members"
            | "profile" | "browse_channels" | "editor" => {
                // Named, bound, and listed in the shortcuts window — but not
                // built yet. Saying so beats a key that does nothing, which
                // reads as a broken client rather than an unfinished one.
                self.say(format!("{name}: not in this build yet"));
            }

            _ => self.message_action(name, sender),
        }
    }

    /// The actions that need a message under the cursor.
    fn message_action(&mut self, name: &str, sender: &ComponentSender<Self>) {
        if let Some(n) = name
            .strip_prefix("workspace_")
            .and_then(|d| d.parse::<usize>().ok())
        {
            if n >= 1 && n <= self.workspaces.len() {
                self.switch_to(n - 1);
            } else {
                self.say(format!("no workspace {n}"));
            }
            return;
        }
        if let Some(n) = name
            .strip_prefix("react_")
            .and_then(|d| d.parse::<usize>().ok())
        {
            match slk_core::emoji::QUICK.get(n - 1) {
                Some(e) => self.react(e, sender),
                None => self.say(format!("no quick reaction {n}")),
            }
            return;
        }

        let Some((_, m)) = self.cursor_message() else {
            self.notice("no message selected — alt-Up picks one".into(), sender);
            return;
        };
        let mine = m.author.id_str() == self.shared.self_id.borrow().as_str();
        let link = crate::logic::first_link(&m.body);
        if let Err(why) = crate::logic::allowed(name, mine, m.files.len(), link.iter().len()) {
            self.notice(why.into(), sender);
            return;
        }
        let Some(ch) = self.open.clone() else { return };

        match name {
            "copy_text" => {
                let names = self.shared.names.borrow();
                let text = m.body.plain_with(&*names);
                drop(names);
                self.to_clipboard(&text);
                self.notice("copied".into(), sender);
            }
            // Asked for, not built here: a permalink is Slack's to mint, and
            // the answer arrives as an event that puts it on the clipboard.
            "copy_link" => self.send(Command::Permalink(ch, m.ts.clone())),
            "open_link" => {
                if let Some(url) = link {
                    if let Err(e) = gtk::gio::AppInfo::launch_default_for_uri(
                        &url,
                        None::<&gtk::gio::AppLaunchContext>,
                    ) {
                        self.notice(format!("could not open: {e}"), sender);
                    }
                }
            }
            "save" => self.send(Command::Save {
                channel: ch,
                ts: m.ts.clone(),
                on: !m.saved,
            }),
            "pin" => self.send(Command::Pin {
                channel: ch,
                ts: m.ts.clone(),
                on: !m.pinned,
            }),
            "download_files" => self.send(Command::DownloadFiles {
                channel: ch,
                ts: m.ts.clone(),
                dir: download_dir(),
            }),
            "edit_message" => {
                // The raw mrkdwn, not the rendered text: an edit that
                // re-sends the rendering strips every link and mention the
                // message had.
                self.composer.buffer().set_text(&m.text);
                self.editing = Some(m.ts.clone());
                self.active = Pane::Conv;
                self.composer.grab_focus();
                self.refresh_hint();
            }
            "delete_message" => self.confirm_delete(ch, m.ts.clone(), sender),
            // What the confirmation dialog sends back. Not bindable, and not
            // in the action list: there is no key that deletes without asking.
            "delete_confirmed" => self.send(Command::Delete(ch, m.ts.clone())),
            other => self.say(format!("unbound action {other}")),
        }
    }
    // ---- history -------------------------------------------------------

    /// Ask for the page before the oldest message on screen.
    fn load_older(&mut self) {
        if self.loading_older || self.at_beginning || self.list.is_empty() {
            return;
        }
        let (Some(ch), Some(first)) = (self.open.clone(), self.list.get(0)) else {
            return;
        };
        let before = first.borrow().msg.ts.clone();
        self.loading_older = true;
        bench::report("scrollback_asked", before.as_str());
        self.loading.set_label("loading earlier messages…");
        self.loading.set_visible(true);
        self.send(Command::LoadOlder(ch, before));
    }

    /// A page of scrollback goes **in front of** what is already there, and
    /// the view must not move under the reader's eyes.
    ///
    /// Two things are easy to get wrong here and both were: appending a page
    /// of older messages to the end (which is what "append_older" invited),
    /// and leaving the row that used to be first grouped as though it still
    /// had nothing above it.
    fn prepend_rows(&mut self, msgs: Vec<slk_core::Message>) {
        if msgs.is_empty() {
            self.at_beginning = true;
            self.loading.set_label("the beginning of the conversation");
            return;
        }
        // Reading upwards is exactly the case where the bottom must not be
        // taken back.
        self.follow_bottom = false;
        let adj = self.scroller.vadjustment();
        let anchor = adj.upper() - adj.value();

        let n = msgs.len() as u32;
        let old_first = self.list.get(0).map(|r| r.borrow().msg.clone());
        for (i, row) in self.make_rows(msgs, false).into_iter().enumerate() {
            self.list.insert(i as u32, row);
        }
        // The message that used to be first now has a predecessor, so its
        // day break and its grouping have to be worked out again.
        if let Some(m) = old_first {
            self.replace_row(Pane::Conv, n, m);
        }

        // Hold the reading position: after the insert the content is taller,
        // so the same distance from the bottom is a different value. An idle
        // callback, because the new rows have not been measured yet.
        let scroller = self.scroller.clone();
        gtk::glib::idle_add_local_once(move || {
            let adj = scroller.vadjustment();
            adj.set_value(adj.upper() - anchor);
        });
    }

    /// A link in a message. A Slack permalink jumps inside the client; every
    /// other link goes to the browser.
    fn follow_link(&mut self, url: &str, sender: &ComponentSender<Self>) {
        if let Some(link) = slk_core::permalink::parse(url) {
            let known = self.convs().iter().any(|c| c.id == link.channel);
            if known {
                self.open_channel(link.channel.clone());
                self.send(Command::JumpToMessage(link.channel, link.ts));
                return;
            }
        }
        if let Err(e) =
            gtk::gio::AppInfo::launch_default_for_uri(url, None::<&gtk::gio::AppLaunchContext>)
        {
            self.notice(format!("could not open: {e}"), sender);
        }
    }

    // ---- composing -----------------------------------------------------

    /// What the user typed, as Slack wants it on the wire.
    fn wire(&self, text: &str) -> String {
        slk_core::outgoing::encode(text, &*self.shared.names.borrow())
    }

    /// The text left of the cursor in the main composer.
    fn before_cursor(&self) -> String {
        let buf = self.composer.buffer();
        let cursor = buf.iter_at_mark(&buf.get_insert());
        buf.text(&buf.start_iter(), &cursor, false).to_string()
    }

    /// Decide whether the completion popup should be up, and with what.
    fn refresh_completions(&mut self) {
        // Only the last line matters, and only up to the cursor: a mention
        // three lines above is finished business.
        let before = self.before_cursor();
        let line = before.rsplit('\n').next().unwrap_or_default().to_string();
        let base = before.len() - line.len();
        let Some(mut c) = crate::logic::completing(&line) else {
            self.close_completions();
            return;
        };
        c.at += base;
        let rows = self.candidates(&c);
        if rows.is_empty() {
            self.close_completions();
            return;
        }

        while let Some(child) = self.complete_list.first_child() {
            self.complete_list.remove(&child);
        }
        self.candidates = rows.iter().map(|(insert, _)| insert.clone()).collect();
        for (_, label) in &rows {
            let l = gtk::Label::new(Some(label));
            l.set_xalign(0.0);
            l.set_margin_start(8);
            l.set_margin_end(8);
            self.complete_list.append(&l);
        }
        if let Some(row) = self.complete_list.row_at_index(0) {
            self.complete_list.select_row(Some(&row));
        }
        self.completing = Some(c);
        self.point_completions();
        self.complete.popup();
    }

    /// Put the popup under the word being completed, not under the widget:
    /// a list anchored to the whole composer points at nothing.
    fn point_completions(&self) {
        let buf = self.composer.buffer();
        let cursor = buf.iter_at_mark(&buf.get_insert());
        let loc = self.composer.iter_location(&cursor);
        let (x, y) =
            self.composer
                .buffer_to_window_coords(gtk::TextWindowType::Widget, loc.x(), loc.y());
        self.complete
            .set_pointing_to(Some(&gtk::gdk::Rectangle::new(x, y, 1, loc.height())));
    }

    fn close_completions(&mut self) {
        self.completing = None;
        self.candidates.clear();
        if self.complete.is_visible() {
            self.complete.popdown();
        }
    }

    /// Replace the token under the cursor with what is selected.
    fn accept_completion(&mut self) {
        let Some(c) = self.completing.take() else {
            return;
        };
        let i = self
            .complete_list
            .selected_row()
            .map(|r| r.index() as usize)
            .unwrap_or(0);
        let Some(insert) = self.candidates.get(i).cloned() else {
            return;
        };
        let buf = self.composer.buffer();
        let mut start = buf.start_iter();
        start.forward_chars(self.before_cursor()[..c.at].chars().count() as i32);
        let cursor = buf.iter_at_mark(&buf.get_insert());
        // The flag, because deleting and inserting each fire `changed`, and
        // the popup would reopen on its own half-finished work.
        self.inserting.set(true);
        buf.delete(&mut start, &mut cursor.clone());
        buf.insert(&mut start, &insert);
        self.inserting.set(false);
        self.close_completions();
    }

    /// What to offer, as (what to insert, what to show).
    fn candidates(&self, c: &crate::logic::Completing) -> Vec<(String, String)> {
        use crate::logic::Complete;
        let q = c.query.to_lowercase();
        let names = self.shared.names.borrow();
        // Prefix matches first: typing "de" should reach #design before
        // #incidents-decided.
        let rank = |name: &str| -> Option<usize> {
            let n = name.to_lowercase();
            if n.starts_with(&q) {
                Some(0)
            } else if q.is_empty() || n.contains(&q) {
                Some(1)
            } else {
                None
            }
        };
        let mut out: Vec<(usize, String, String)> = match c.kind {
            Complete::User => {
                let mut v: Vec<(usize, String, String)> = names
                    .users
                    .values()
                    .filter_map(|label| {
                        rank(label).map(|r| (r, format!("@{label} "), format!("@{label}")))
                    })
                    .collect();
                v.extend(names.groups.keys().filter_map(|h| {
                    rank(h).map(|r| (r, format!("@{h} "), format!("@{h}  ·  group")))
                }));
                v.extend(["here", "channel", "everyone"].iter().filter_map(|b| {
                    rank(b).map(|r| {
                        (
                            r + 1,
                            format!("@{b} "),
                            format!("@{b}  ·  notifies the conversation"),
                        )
                    })
                }));
                v
            }
            Complete::Channel => names
                .channels
                .values()
                .filter_map(|name| rank(name).map(|r| (r, format!("#{name} "), format!("#{name}"))))
                .collect(),
            // Ranked by position, not by name: `search_with` already put
            // the best first, and re-sorting these by label length offered
            // :rock: ahead of :rocket:.
            Complete::Emoji => slk_core::emoji::search_with(&c.query, 24, self.skin)
                .into_iter()
                .enumerate()
                .map(|(i, (name, glyph))| (i, format!(":{name}: "), format!("{glyph}  :{name}:")))
                .collect(),
            Complete::Command => crate::logic::COMMANDS
                .iter()
                .filter_map(|(cmd, help)| {
                    rank(&cmd[1..]).map(|r| (r, format!("{cmd} "), format!("{cmd}  ·  {help}")))
                })
                .collect(),
        };
        out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.len().cmp(&b.2.len())));
        out.truncate(24);
        out.into_iter().map(|(_, i, l)| (i, l)).collect()
    }

    /// Keep what was typed and not sent, per conversation.
    fn stash_draft(&mut self) {
        let Some(ch) = self.open.clone() else { return };
        let buf = self.composer.buffer();
        let (a, b) = buf.bounds();
        let text = buf.text(&a, &b, false).to_string();
        if text.trim().is_empty() {
            self.drafts.remove(&ch);
        } else {
            self.drafts.insert(ch, text);
        }
    }

    // ---- the message cursor -------------------------------------------

    fn list_of(&self, pane: Pane) -> &TypedListView<Row, gtk::SingleSelection> {
        match pane {
            Pane::Thread if self.thread.is_some() => &self.thread_list,
            _ => &self.list,
        }
    }

    fn position_in(&self, pane: Pane, ts: &str) -> Option<u32> {
        self.list_of(pane)
            .iter()
            .position(|i| i.borrow().msg.ts.as_str() == ts)
            .map(|i| i as u32)
    }

    /// The message under the cursor, and where it is.
    fn cursor_message(&self) -> Option<(u32, slk_core::Message)> {
        let list = self.list_of(self.active);
        let sel = list.selection_model.selected();
        if sel == gtk::INVALID_LIST_POSITION {
            return None;
        }
        list.get(sel).map(|r| (sel, r.borrow().msg.clone()))
    }

    fn set_cursor(&self, to: Option<usize>) {
        let Some(to) = to else { return };
        let list = self.list_of(self.active);
        if to as u32 >= list.len() {
            return;
        }
        list.selection_model.set_selected(to as u32);
        // A selection that scrolled off screen is not a cursor.
        list.view
            .scroll_to(to as u32, gtk::ListScrollFlags::NONE, None);
    }

    fn move_cursor(&mut self, delta: isize) {
        let list = self.list_of(self.active);
        let sel = list.selection_model.selected();
        let at = (sel != gtk::INVALID_LIST_POSITION).then_some(sel as usize);
        let to = crate::logic::cursor(at, list.len() as usize, delta);
        self.set_cursor(to);
    }

    /// The newest message in the conversation, for marking read.
    fn newest(&self) -> Option<slk_core::Ts> {
        self.list
            .len()
            .checked_sub(1)
            .and_then(|i| self.list.get(i))
            .map(|r| r.borrow().msg.ts.clone())
    }

    /// Put a rebuilt row back where it was, keeping the cursor on it.
    ///
    /// `TypedListView` binds on demand, so mutating the model behind a bound
    /// widget changes nothing on screen; the row has to be replaced.
    fn replace_row(&mut self, pane: Pane, pos: u32, msg: slk_core::Message) {
        let list = match pane {
            Pane::Thread if self.thread.is_some() => &mut self.thread_list,
            _ => &mut self.list,
        };
        let Some(old) = list.get(pos) else { return };
        let (meta, shared) = {
            let r = old.borrow();
            (
                crate::logic::Meta {
                    grouped: r.meta.grouped,
                    day_break: r.meta.day_break.clone(),
                    unread_break: r.meta.unread_break,
                },
                r.shared.clone(),
            )
        };
        list.remove(pos);
        list.insert(pos, Row { msg, meta, shared });
        list.selection_model.set_selected(pos);
        // Removing and re-inserting can take the row out of the viewport —
        // measured: reacting to the newest message scrolled it off screen and
        // the chip appeared somewhere nobody was looking. The cursor has to
        // stay where the eye is.
        list.view.scroll_to(pos, gtk::ListScrollFlags::NONE, None);
    }

    /// Toggle a reaction on the message under the cursor.
    fn react(&mut self, name: &str, sender: &ComponentSender<Self>) {
        let Some((pos, mut m)) = self.cursor_message() else {
            self.notice("no message selected — alt-Up picks one".into(), sender);
            return;
        };
        let Some(ch) = self.open.clone() else { return };
        if self.read_only {
            self.notice("read-only: nothing is sent".into(), sender);
            return;
        }
        let toned = slk_core::emoji::with_tone(name, self.skin);
        let on = crate::logic::toggle_reaction(&mut m.reactions, &toned);
        let ts = m.ts.clone();
        let pane = self.active;
        self.replace_row(pane, pos, m);
        self.send(Command::React {
            channel: ch,
            ts,
            name: toned,
            on,
        });
    }

    // ---- threads -------------------------------------------------------

    fn open_thread(&mut self, sender: &ComponentSender<Self>) {
        let Some((_, m)) = self.cursor_message() else {
            self.notice("no message selected — alt-Up picks one".into(), sender);
            return;
        };
        let Some(ch) = self.open.clone() else { return };
        // A reply opens its own parent's thread, not a thread of its own.
        let parent = m.thread_ts.clone().unwrap_or_else(|| m.ts.clone());
        self.thread = Some(parent.clone());
        self.thread_list.clear();
        self.thread_title.set_label("Thread");
        self.follow.set_active(m.subscribed);
        self.refresh_follow_label();
        self.thread_pane.set_visible(true);
        // Measured from the left, so the pane's own width has to be taken
        // off the current split rather than set on it — and never more than
        // half, or on a tiled window the thread squeezes the conversation it
        // is a reply to down to a column of two words.
        let w = self.conv_paned.width();
        if w > 320 {
            self.conv_paned.set_position(w - 380.min(w / 2));
        }
        self.active = Pane::Thread;
        self.send(Command::OpenThread(ch, parent));
        self.thread_composer.grab_focus();
    }

    fn close_thread(&mut self) {
        self.thread = None;
        self.thread_list.clear();
        self.thread_pane.set_visible(false);
        self.active = Pane::Conv;
        self.composer.grab_focus();
    }

    fn refresh_follow_label(&self) {
        self.follow.set_label(if self.follow.is_active() {
            "Following"
        } else {
            "Follow"
        });
    }

    /// What Escape closes, innermost first.
    fn escape(&mut self) {
        if self.complete.is_visible() {
            self.close_completions();
        } else if self.picker.is_some() {
            self.close_picker();
        } else if gtk::prelude::WidgetExt::is_visible(&self.jump) {
            self.close_jump();
        } else if self.editing.is_some() {
            self.editing = None;
            self.composer.buffer().set_text("");
            self.refresh_hint();
        } else if self.thread.is_some() {
            self.close_thread();
        } else {
            self.list
                .selection_model
                .set_selected(gtk::INVALID_LIST_POSITION);
            self.composer.grab_focus();
        }
    }

    // ---- windows the actions open --------------------------------------

    fn window(&self) -> Option<gtk::Window> {
        self.composer.root().and_downcast::<gtk::Window>()
    }

    /// The emoji picker: one window rather than a popover per row, because
    /// the keyboard has no anchor to hang a popover from and two
    /// implementations of the same picker is one too many.
    fn open_picker(&mut self, sender: &ComponentSender<Self>) {
        if self.cursor_message().is_none() {
            self.notice("no message selected — alt-Up picks one".into(), sender);
            return;
        }
        self.close_picker();
        let win = gtk::Window::new();
        win.set_title(Some("Add a reaction"));
        win.set_modal(true);
        win.set_transient_for(self.window().as_ref());
        win.set_default_size(360, 320);
        win.add_css_class("picker");

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
        outer.set_margin_top(8);
        outer.set_margin_bottom(8);
        outer.set_margin_start(8);
        outer.set_margin_end(8);
        let entry = gtk::SearchEntry::new();
        entry.set_placeholder_text(Some("search emoji…"));
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_vexpand(true);
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::Single);
        flow.set_max_children_per_line(8);
        scroller.set_child(Some(&flow));
        outer.append(&entry);
        outer.append(&scroller);
        win.set_child(Some(&outer));

        let skin = self.skin;
        let s = sender.input_sender().clone();
        let fill = move |flow: &gtk::FlowBox, query: &str| {
            while let Some(c) = flow.first_child() {
                flow.remove(&c);
            }
            for (name, glyph) in slk_core::emoji::search_with(query, 64, skin) {
                let b = gtk::Button::new();
                b.set_child(Some(&gtk::Label::new(Some(&glyph))));
                b.set_tooltip_text(Some(&format!(":{name}:")));
                b.add_css_class("flat");
                // The name, not the glyph: Slack reacts by shortcode, and
                // the tone is added when it is sent.
                b.set_widget_name(&name);
                let s = s.clone();
                b.connect_clicked(move |b| {
                    let _ = s.send(Msg::Picked(b.widget_name().to_string()));
                });
                flow.append(&b);
            }
        };
        fill(&flow, "");
        {
            let flow2 = flow.clone();
            let fill = fill.clone();
            entry.connect_search_changed(move |e| fill(&flow2, &e.text()));
        }
        // Enter takes the first match, which is the whole point of typing a
        // name; arrow-then-Enter takes the one under the cursor. Without
        // both, the picker is mouse-only and every reaction costs a reach.
        {
            let flow2 = flow.clone();
            entry.connect_activate(move |_| {
                if let Some(b) = flow2
                    .child_at_index(0)
                    .and_then(|c| c.child())
                    .and_downcast::<gtk::Button>()
                {
                    b.emit_clicked();
                }
            });
        }
        flow.connect_child_activated(|_, child| {
            if let Some(b) = child.child().and_downcast::<gtk::Button>() {
                b.emit_clicked();
            }
        });
        {
            // Escape here, not only on the main window: a modal window has
            // the keyboard, so the window's accelerators never see the key.
            let k = gtk::EventControllerKey::new();
            let s = sender.input_sender().clone();
            k.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    let _ = s.send(Msg::Action("normal".into()));
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            });
            win.add_controller(k);
        }
        win.present();
        entry.grab_focus();
        self.picker = Some(win);
    }

    fn close_picker(&mut self) {
        if let Some(w) = self.picker.take() {
            w.close();
        }
    }

    /// Deleting is the one action with no undo, so it asks.
    fn confirm_delete(
        &mut self,
        channel: ChannelId,
        ts: slk_core::Ts,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = gtk::AlertDialog::builder()
            .message("Delete this message?")
            .detail("It disappears for everyone, and there is no undo.")
            .buttons(["Cancel", "Delete"])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let s = sender.input_sender().clone();
        let at = ts.as_str().to_string();
        let _ = channel;
        dialog.choose(
            self.window().as_ref(),
            gtk::gio::Cancellable::NONE,
            move |r| {
                if r == Ok(1) {
                    let _ = s.send(Msg::RowAction {
                        at,
                        act: "delete_confirmed".into(),
                    });
                }
            },
        );
    }

    fn pick_file(&mut self, sender: &ComponentSender<Self>) {
        let Some(ch) = self.open.clone() else { return };
        if self.read_only {
            self.notice("read-only: nothing is sent".into(), sender);
            return;
        }
        let thread = self.thread.clone();
        let tx = self.ws().map(|w| w.commands.clone());
        let rt = self.runtime.clone();
        let dialog = gtk::FileDialog::builder().title("Send a file").build();
        dialog.open(
            self.window().as_ref(),
            gtk::gio::Cancellable::NONE,
            move |r| {
                let Ok(file) = r else { return };
                let Some(path) = file.path() else { return };
                if let Some(tx) = tx {
                    rt.spawn(async move {
                        let _ = tx
                            .send(Command::UploadFile {
                                channel: ch,
                                thread,
                                path,
                                comment: None,
                            })
                            .await;
                    });
                }
            },
        );
    }

    /// The shortcuts window, generated from the live keymap.
    ///
    /// Written by hand it drifts from the bindings within a month; generated
    /// it cannot, and an action with no free key says so instead of being
    /// quietly missing.
    fn show_shortcuts(&mut self) {
        let win = gtk::Window::new();
        win.set_title(Some("Keyboard shortcuts"));
        win.set_transient_for(self.window().as_ref());
        win.set_default_size(460, 560);
        win.add_css_class("picker");
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 4);
        outer.set_margin_top(10);
        outer.set_margin_bottom(10);
        outer.set_margin_start(12);
        outer.set_margin_end(12);

        let mut groups: Vec<&'static str> = Vec::new();
        for b in &self.bindings {
            if !groups.contains(&b.action.group()) {
                groups.push(b.action.group());
            }
        }
        for g in groups {
            let head = gtk::Label::new(Some(&g.to_uppercase()));
            head.set_xalign(0.0);
            head.add_css_class("section");
            head.set_margin_top(10);
            outer.append(&head);
            for b in self.bindings.iter().filter(|b| b.action.group() == g) {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                let key = gtk::Label::new(Some(if b.accel.is_empty() {
                    "—"
                } else {
                    b.accel.as_str()
                }));
                key.set_xalign(1.0);
                key.set_size_request(150, -1);
                key.add_css_class("chip");
                let what = gtk::Label::new(Some(b.action.help()));
                what.set_xalign(0.0);
                what.set_hexpand(true);
                line.append(&key);
                line.append(&what);
                outer.append(&line);
            }
        }
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_child(Some(&outer));
        win.set_child(Some(&scroller));
        {
            let k = gtk::EventControllerKey::new();
            let w = win.clone();
            k.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    w.close();
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            });
            win.add_controller(k);
        }
        win.present();
    }

    // ---- small things --------------------------------------------------

    fn composer_view(&self) -> &gtk::TextView {
        match self.active {
            Pane::Thread if self.thread.is_some() => &self.thread_composer,
            _ => &self.composer,
        }
    }

    fn to_clipboard(&self, text: &str) {
        if let Some(d) = gtk::gdk::Display::default() {
            d.clipboard().set_text(text);
        }
    }

    fn refresh_hint(&mut self) {
        self.hint = if self.read_only {
            "read-only — nothing is sent".into()
        } else if self.editing.is_some() {
            "editing — enter saves · esc cancels".into()
        } else {
            "enter sends · shift+enter newline".into()
        };
    }

    /// Back and forward over the conversations visited.
    fn go(&mut self, forward: bool) {
        let to = if forward {
            self.history_at + 1
        } else {
            match self.history_at.checked_sub(1) {
                Some(i) => i,
                None => return,
            }
        };
        let Some(id) = self.history.get(to).cloned() else {
            return;
        };
        self.history_at = to;
        self.visit(id.clone());
        if let Some(i) = self.convs().iter().position(|c| c.id == id) {
            self.select_conv(i);
        }
    }

    fn rebuild_sidebar(&mut self) {
        while let Some(child) = self.sidebar.first_child() {
            self.sidebar.remove(&child);
        }
        self.row_map.clear();
        let convs: Vec<SidebarEntry> = self.convs().to_vec();

        for (sec, title) in SECTIONS {
            let members: Vec<(usize, &SidebarEntry)> = convs
                .iter()
                .enumerate()
                .filter(|(_, c)| section_of(c) == sec)
                .collect();
            if members.is_empty() {
                continue;
            }
            let folded = self.collapsed.contains(&sec);

            // A heading that folds, the way an editor's panels do. It is a
            // row, so it takes an index — hence `row_map`.
            let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            head.add_css_class("section");
            head.append(&gtk::Label::new(Some(if folded { "▸" } else { "▾" })));
            head.append(&gtk::Label::new(Some(title)));
            if folded {
                let n = gtk::Label::new(Some(&members.len().to_string()));
                n.add_css_class("count");
                n.set_margin_start(4);
                head.append(&n);
            }
            let hrow = gtk::ListBoxRow::new();
            hrow.set_child(Some(&head));
            hrow.set_selectable(false);
            hrow.set_activatable(true);
            let click = gtk::GestureClick::new();
            let s = self.shared.sender.clone();
            click.connect_released(move |_, _, _, _| {
                let _ = s.send(Msg::ToggleSection(sec));
            });
            hrow.add_controller(click);
            self.sidebar.append(&hrow);
            self.row_map.push(None);
            if folded {
                continue;
            }

            for (i, c) in members {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                row.add_css_class("conv");
                let name = gtk::Label::new(None);
                name.set_xalign(0.0);
                name.set_hexpand(true);
                name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                let label = if c.is_dm() {
                    format!("  {}", c.name)
                } else if c.is_private() {
                    format!("🔒 {}", c.name)
                } else {
                    format!("#  {}", c.name)
                };
                name.set_label(&label);
                if c.has_unread() {
                    name.add_css_class("unread");
                }
                row.append(&name);

                // One accessible name for the row, so a screen reader says
                // "engineering, 1 mention" rather than reading two labels.
                // GTK does not take a list item's name from its own label
                // property — measured — but a Label honours it.
                let spoken = if c.mentions > 0 {
                    format!(
                        "{label}, {} mention{}",
                        c.mentions,
                        if c.mentions == 1 { "" } else { "s" }
                    )
                } else if c.unread > 0 {
                    format!("{label}, {} unread", c.unread)
                } else {
                    label.clone()
                };
                name.update_property(&[gtk::accessible::Property::Label(&spoken)]);

                if c.mentions > 0 {
                    let b = gtk::Label::new(Some(&c.mentions.to_string()));
                    b.add_css_class("badge");
                    row.append(&b);
                } else if c.unread > 0 {
                    let b = gtk::Label::new(Some(&c.unread.to_string()));
                    b.add_css_class("count");
                    row.append(&b);
                }
                self.sidebar.append(&row);
                self.row_map.push(Some(i));
            }
        }

        // Keep the open conversation selected across a rebuild, or the
        // sidebar loses its place every time a badge changes.
        if let Some(open) = &self.open {
            if let Some(conv_i) = convs.iter().position(|c| &c.id == open) {
                if let Some(row_i) = self.row_map.iter().position(|m| *m == Some(conv_i)) {
                    if let Some(row) = self.sidebar.row_at_index(row_i as i32) {
                        self.sidebar.select_row(Some(&row));
                    }
                }
            }
        }
        self.refresh_status_right();
    }

    /// The workspace rail. Rebuilt when a workspace connects or the current
    /// one changes; there are never more than a handful.
    fn rebuild_rail(&mut self) {
        while let Some(child) = self.rail.first_child() {
            self.rail.remove(&child);
        }
        for (i, w) in self.workspaces.iter().enumerate() {
            let tile = gtk::Label::new(Some(
                &w.name
                    .chars()
                    .find(|c| c.is_alphanumeric())
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".into()),
            ));
            tile.add_css_class("tile");
            tile.add_css_class(&format!("avatar-{}", crate::row::colour_slot(&w.name)));
            let b = gtk::Button::new();
            b.set_child(Some(&tile));
            b.set_tooltip_text(Some(&format!(
                "{}{}",
                w.name,
                if w.connected { "" } else { " (connecting…)" }
            )));
            b.update_property(&[gtk::accessible::Property::Label(&w.name)]);
            if i == self.current {
                b.add_css_class("current");
            }
            let s = self.shared.sender.clone();
            b.connect_clicked(move |_| {
                let _ = s.send(Msg::Action(format!("workspace_{}", i + 1)));
            });
            self.rail.append(&b);
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
                    bench::reset_frames();
                    let targets = [
                        0u32,
                        total / 2,
                        total.saturating_sub(1),
                        total / 4,
                        (total * 3) / 4,
                    ];
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
        for w in &self.workspaces {
            let tx = w.commands.clone();
            self.runtime.spawn(async move {
                let _ = tx.send(Command::Shutdown).await;
            });
        }
    }
}

/// "Today", "Yesterday", a weekday within the week, else the date. The one
/// place the client formats a day, so the separator and anything else that
/// needs one agree.
/// Where downloaded files go: the user's own download directory, or their
/// home if the desktop has not told us where that is.
fn download_dir() -> std::path::PathBuf {
    gtk::glib::user_special_dir(gtk::glib::UserDirectory::Downloads)
        .or_else(|| Some(gtk::glib::home_dir()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

pub fn day_label(secs: i64) -> String {
    let Ok(ts) = jiff::Timestamp::from_second(secs) else {
        return String::new();
    };
    let tz = jiff::tz::TimeZone::system();
    let day = ts.to_zoned(tz.clone()).date();
    let today = jiff::Zoned::now().with_time_zone(tz).date();
    match (today - day).get_days() {
        0 => "TODAY".to_string(),
        1 => "YESTERDAY".to_string(),
        d if (2..7).contains(&d) => day.strftime("%A").to_string().to_uppercase(),
        _ => day.strftime("%A, %-d %B").to_string().to_uppercase(),
    }
}
