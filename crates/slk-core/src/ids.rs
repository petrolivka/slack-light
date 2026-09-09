//! Identifiers, and the one type that is easy to get wrong.
//!
//! Every id is a newtype rather than a `String`, because this codebase passes
//! four kinds of opaque Slack id around and a channel where a user belongs is a
//! bug the compiler should catch.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }
        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

id_type!(
    TeamId,
    "A workspace. `T…`, or `E…` for an Enterprise Grid org."
);
id_type!(UserId, "A person or bot user. `U…`, or `W…` on Grid.");
id_type!(
    ChannelId,
    "Any conversation: `C…` channel, `D…` DM, `G…` group."
);
id_type!(BotId, "A bot identity. `B…`");
id_type!(FileId, "An uploaded file. `F…`");
id_type!(SubteamId, "A user group. `S…`");

/// A Slack message timestamp: `"1725700000.123456"`.
///
/// It is the message's primary key within a channel and, because Slack
/// zero-pads both halves, it sorts lexically in time order. That property is
/// load-bearing — history paging, the unread cursor and gap detection all
/// compare these as strings rather than parsing them.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Ts(String);

impl Ts {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Whole seconds since the epoch, for formatting a clock time.
    pub fn secs(&self) -> i64 {
        self.0
            .split('.')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
}

impl From<&str> for Ts {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}
impl fmt::Display for Ts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Debug for Ts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ts({})", self.0)
    }
}

/// A message, qualified by the workspace and channel that own it.
///
/// Nothing in this codebase identifies a message by `ts` alone: the same
/// timestamp exists in every workspace, and several workspaces are connected at
/// once.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct MsgRef {
    pub team: TeamId,
    pub channel: ChannelId,
    pub ts: Ts,
}

impl MsgRef {
    pub fn new(team: TeamId, channel: ChannelId, ts: Ts) -> Self {
        Self { team, channel, ts }
    }
}
