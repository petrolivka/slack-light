//! Domain types and the two message parsers.
//!
//! No I/O, no async, no terminal. That is deliberate: the parsers are the part
//! most exposed to Slack changing shape underneath us, and keeping them pure
//! means they can be developed against fixtures, fuzzed, and asserted on
//! without a network or an account anywhere near them.

pub mod ast;
pub mod blocks;
pub mod emoji;
pub mod ids;
pub mod model;
pub mod mrkdwn;
pub mod permalink;
pub mod richtext;

pub use ast::{Doc, Inline, Names, Style};
pub use ids::{BotId, ChannelId, FileId, MsgRef, SubteamId, TeamId, Ts, UserId};
pub use model::{
    Author, Conversation, ConversationKind, Delivery, Message, Presence, Reaction, User, Workspace,
};
