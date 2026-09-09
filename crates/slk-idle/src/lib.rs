//! Away when the desktop is idle, and honest when it cannot tell.
//!
//! FR-X6 asks for `ext-idle-notify` on Wayland with a logind fallback. Both
//! halves matter, and so does the third case FR-K7 insists on: "the
//! compositor did not answer" is not the same as "you are not idle", and a
//! client that reports one as the other is why somebody's colleagues think
//! they are at their desk at three in the morning.
//!
//! This opens its own Wayland connection rather than borrowing GTK's. A
//! second connection from one process is ordinary — it is what a client
//! library does — and it keeps every wayland call on this thread instead of
//! on the one drawing the window.

use std::sync::mpsc::Sender;
use std::time::Duration;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

/// What the desktop said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idle {
    /// Nothing has happened for the configured time.
    Away,
    /// Somebody touched the machine again.
    Back,
}

/// Whether this desktop can answer the question at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// Watching. Events will arrive on the channel.
    Watching,
    /// Asked, and told no — the compositor does not implement the protocol.
    Unsupported(&'static str),
    /// Could not ask: no Wayland session, or the connection failed.
    Unknown(String),
}

impl Support {
    /// One line for `--doctor`, in the three-way form FR-K7 requires.
    pub fn describe(&self) -> String {
        match self {
            Support::Watching => "yes — ext-idle-notify-v1".into(),
            Support::Unsupported(why) => format!("no — {why}"),
            Support::Unknown(why) => format!("did not answer — {why}"),
        }
    }
}

struct State {
    seat: Option<wl_seat::WlSeat>,
    notifier: Option<ExtIdleNotifierV1>,
    tx: Sender<Idle>,
}

/// Watch this session, and report going idle and coming back.
///
/// Returns as soon as it knows whether it can watch; the watching itself
/// happens on a thread of its own, which ends when the receiver is dropped.
pub fn watch(after: Duration, tx: Sender<Idle>) -> Support {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Support::Unknown("not a Wayland session".into());
    }
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => return Support::Unknown(e.to_string()),
    };

    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    let display = conn.display();
    display.get_registry(&qh, ());

    let mut state = State {
        seat: None,
        notifier: None,
        tx,
    };
    // One round trip is enough to see the whole registry: the compositor
    // advertises every global before the first sync completes.
    if let Err(e) = queue.roundtrip(&mut state) {
        return Support::Unknown(e.to_string());
    }

    let (Some(notifier), Some(seat)) = (state.notifier.clone(), state.seat.clone()) else {
        return Support::Unsupported("the compositor does not offer ext-idle-notify-v1");
    };

    // Milliseconds, and the protocol treats zero as "immediately", which is
    // not a setting anybody wants by accident.
    let ms = after.as_millis().clamp(1_000, u32::MAX as u128) as u32;
    notifier.get_idle_notification(ms, &seat, &qh, ());

    std::thread::Builder::new()
        .name("slk-idle".into())
        .spawn(move || loop {
            if queue.blocking_dispatch(&mut state).is_err() {
                return;
            }
        })
        .map(|_| Support::Watching)
        .unwrap_or_else(|e| Support::Unknown(e.to_string()))
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_seat" if state.seat.is_none() => {
                state.seat = Some(registry.bind(name, version.min(7), qh, ()));
            }
            "ext_idle_notifier_v1" if state.notifier.is_none() => {
                state.notifier = Some(registry.bind(name, version.min(1), qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // A failed send means the window has gone. Nothing to do about it
        // here; the thread ends when the queue does.
        let _ = match event {
            ext_idle_notification_v1::Event::Idled => state.tx.send(Idle::Away),
            ext_idle_notification_v1::Event::Resumed => state.tx.send(Idle::Back),
            _ => Ok(()),
        };
    }
}

// Neither of these sends events we act on; the protocol still requires a
// dispatcher for each.
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
