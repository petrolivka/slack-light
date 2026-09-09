//! The window.
//!
//! relm4 components over the sync engine. The rule the whole crate is built
//! around: **the GTK main thread never does I/O.** It holds channel ends and
//! widgets, sends a `Command`, applies the `Event` that comes back, and
//! renders what it has meanwhile. The engine lives on a tokio runtime on
//! other threads, built by the binary before GTK is initialised.
//!
//! Decisions — which conversation to land on, where an upserted row goes,
//! how a chord becomes an accelerator — live in `logic`, as plain functions
//! with tests, because relm4's `ComponentSender` cannot be constructed in a
//! test and `update()` should only ever dispatch.

pub mod app;
pub mod bench;
pub mod blockkit;
pub mod keys;
pub mod logic;
pub mod markup;
pub mod row;

pub use app::{Init, Workspace};

/// Run the application. Returns when the window is closed.
pub fn run(init: Init) {
    // GTK would otherwise parse the process's own arguments and refuse ours.
    let app = relm4::RelmApp::new("dev.olivka.slack_light").with_args(Vec::new());
    app.run::<app::App>(init);
}
