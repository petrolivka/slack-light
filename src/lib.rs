//! The pieces the binary and its development tools share.
//!
//! `slack-light` is a binary; the probes under `src/bin/` are gated behind
//! the `dev-tools` feature so they never land in a user's PATH, and they
//! reach the credential store through the same path the binary does.

pub mod doctor;
pub mod ipc;

pub use slk_auth as auth;
