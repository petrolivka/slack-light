//! Errors that callers can act on without matching strings.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// The token or cookie is wrong, expired, or revoked. The only recovery is
    /// the user signing in again.
    Auth,
    /// Slack asked us to slow down. `retry_after` is its number, in seconds.
    RateLimited {
        retry_after: u64,
    },
    NotFound,
    Permission,
    /// The network, TLS, or a timeout — anything worth retrying.
    Transport,
    /// The response parsed as JSON but not as what we expected. This is the
    /// interesting one: it means Slack changed shape.
    Shape,
    Other,
}

#[derive(Debug, Clone)]
pub struct SlackError {
    pub method: String,
    pub kind: ErrorKind,
    pub detail: String,
}

impl SlackError {
    pub fn new(method: impl Into<String>, kind: ErrorKind, detail: impl Into<String>) -> Self {
        SlackError {
            method: method.into(),
            kind,
            detail: detail.into(),
        }
    }

    /// Map Slack's own `error` string onto something a caller can branch on.
    pub fn from_slack(method: &str, code: &str) -> Self {
        let kind = match code {
            "invalid_auth" | "not_authed" | "token_revoked" | "account_inactive"
            | "token_expired" => ErrorKind::Auth,
            "ratelimited" | "rate_limited" => ErrorKind::RateLimited { retry_after: 30 },
            "channel_not_found" | "user_not_found" | "message_not_found" | "thread_not_found"
            | "draft_not_found" => {
                ErrorKind::NotFound
            }
            "not_in_channel"
            | "restricted_action"
            | "missing_scope"
            | "no_permission"
            | "cant_update_message"
            | "cant_delete_message"
            // Archiving and renaming: #general cannot be archived, a direct
            // message cannot be archived or renamed, and on many workspaces
            // only admins may do either.
            | "cant_archive_general"
            | "method_not_supported_for_channel_type"
            | "not_authorized"
            | "restricted_action_non_threadable_channel" => ErrorKind::Permission,
            _ => ErrorKind::Other,
        };
        SlackError::new(method, kind, code)
    }

    /// Is retrying this by itself ever going to work?
    pub fn is_transient(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::Transport | ErrorKind::RateLimited { .. }
        )
    }

    /// What the user should see. Never the raw code, which means nothing to
    /// them, and never the request, which may contain their message.
    pub fn user_message(&self) -> String {
        match &self.kind {
            ErrorKind::Auth => "your session has expired — sign in again".into(),
            ErrorKind::RateLimited { retry_after } => {
                format!("Slack asked us to wait {retry_after}s")
            }
            ErrorKind::NotFound => "that no longer exists".into(),
            ErrorKind::Permission if self.detail == "not_pinnable" => {
                "that kind of message cannot be pinned".into()
            }
            ErrorKind::Permission if self.detail == "cant_archive_general" => {
                "the workspace's #general cannot be archived".into()
            }
            ErrorKind::Permission if self.detail == "method_not_supported_for_channel_type" => {
                "not for this kind of conversation".into()
            }
            ErrorKind::Permission => "you cannot do that here".into(),
            ErrorKind::Transport => "cannot reach Slack".into(),
            ErrorKind::Shape => "Slack sent something unexpected (logged)".into(),
            // The two a person can fix by choosing again, said as such rather
            // than as Slack's code for them.
            ErrorKind::Other if self.detail == "name_taken" => {
                "that name is taken — archived channels keep theirs".into()
            }
            ErrorKind::Other if self.detail.starts_with("invalid_name") => {
                "Slack will not take that name".into()
            }
            ErrorKind::Other => format!("Slack said: {}", self.detail),
        }
    }
}

impl fmt::Display for SlackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?} ({})", self.method, self.kind, self.detail)
    }
}

impl std::error::Error for SlackError {}

pub type Result<T> = std::result::Result<T, SlackError>;
