//! Everything that talks to Slack, behind one trait.
//!
//! Two backends implement it: a browser session, which is what the product is
//! built on, and a mock with no network at all, which is what every test and
//! `--anonymous` use. An official OAuth backend is the third, and the reason
//! the trait exists at all.

pub mod backend;
pub mod error;
pub mod events;
pub mod mock;
pub mod session;

pub use backend::{Boot, Capabilities, CountEntry, Counts, HistoryQuery, Page, SlackBackend};
pub use error::{ErrorKind, Result, SlackError};
pub use events::{EventStream, RtEvent};
pub use mock::MockBackend;
pub use session::{Credentials, SessionBackend};
