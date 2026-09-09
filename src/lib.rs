//! The pieces the binary and its development tools share.
//!
//! `slack-light` is a binary, but its credential handling is also wanted by
//! the probes under `src/bin/`, which are gated behind the `dev-tools`
//! feature so they never land in a user's PATH.

pub mod auth;
