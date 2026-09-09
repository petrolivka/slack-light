//! Every bindable action, by name.
//!
//! Keys map to these rather than to code, which is what makes the config
//! remappable and lets the help overlay be generated from the live keymap. A
//! help screen written by hand drifts from the bindings within a month; one
//! generated from them cannot.

use std::fmt;
use std::str::FromStr;

macro_rules! actions {
    ($($variant:ident => $name:literal, $help:literal, $group:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Action { $($variant),* }

        impl Action {
            pub const ALL: &'static [Action] = &[$(Action::$variant),*];

            pub fn name(self) -> &'static str {
                match self { $(Action::$variant => $name),* }
            }
            /// One line for the help overlay and the command palette.
            pub fn help(self) -> &'static str {
                match self { $(Action::$variant => $help),* }
            }
            pub fn group(self) -> &'static str {
                match self { $(Action::$variant => $group),* }
            }
        }

        impl FromStr for Action {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($name => Ok(Action::$variant),)*
                    other => Err(format!("no action named {other:?}")),
                }
            }
        }
    };
}

actions! {
    Quit           => "quit",            "Quit",                                 "global";
    Help           => "help",            "Show this help",                       "global";
    Palette        => "palette",         "Run any action by name",               "global";
    ToggleSection  => "toggle_section",  "Fold or unfold this sidebar section",  "navigation";
    BrowseChannels => "browse_channels", "Find a channel to join",              "navigation";
    FollowThread   => "follow_thread",   "Follow or stop following a thread",    "threads";
    ToggleBroadcast=> "broadcast",       "Also send this reply to the channel",   "threads";
    GoBack         => "back",            "Back to the previous conversation",    "navigation";
    GoForward      => "forward",         "Forward again",                        "navigation";
    Suspend        => "suspend",         "Give the terminal back to the shell",  "global";
    Redraw         => "redraw",          "Redraw the screen",                    "global";
    ToggleSidebar  => "toggle_sidebar",  "Show or hide the sidebar",             "global";
    FocusNext      => "focus_next",      "Cycle pane focus",                     "global";
    FocusPrev      => "focus_prev",      "Cycle pane focus backwards",           "global";
    JumpTo         => "jump_to",         "Jump to a conversation",               "global";
    NextWorkspace  => "next_workspace",  "Next workspace",                       "global";
    PrevWorkspace  => "prev_workspace",  "Previous workspace",                   "global";
    Workspace1     => "workspace_1",     "Switch to the first workspace",        "global";
    Workspace2     => "workspace_2",     "Switch to the second",                 "global";
    Workspace3     => "workspace_3",     "Switch to the third",                  "global";
    Workspace4     => "workspace_4",     "Switch to the fourth",                 "global";
    Workspace5     => "workspace_5",     "Switch to the fifth",                  "global";
    Search         => "search",          "Search Slack",                         "global";
    Threads        => "threads",         "Threads with replies",                 "global";
    Saved          => "saved",           "Messages kept for later",              "global";
    Mentions       => "mentions",        "Messages that name you",               "global";
    Members        => "members",         "Who is in this conversation",          "global";
    Profile        => "profile",         "Who wrote this",                       "messages";
    Save           => "save",            "Keep this for later",                  "messages";
    Pin            => "pin",             "Pin this to the conversation",         "messages";
    SearchLocal    => "search_local",    "Search what is already downloaded",    "global";
    Pinned         => "pinned",          "What is pinned in this conversation",  "global";

    Star           => "star",            "Star or unstar this conversation",     "conversation";
    Mute           => "mute",            "Mute or unmute this conversation",     "conversation";
    SetTopic       => "set_topic",       "Set the conversation's topic",         "conversation";
    SetPurpose     => "set_purpose",     "Set the conversation's purpose",       "conversation";
    InviteUser     => "invite",          "Invite somebody here",                 "conversation";
    LeaveChannel   => "leave_channel",   "Leave this conversation",              "conversation";

    NextConv       => "next_conversation",   "Next conversation",                "navigate";
    PrevConv       => "prev_conversation",   "Previous conversation",            "navigate";
    NextUnread     => "next_unread",         "Next conversation with unread",    "navigate";
    PrevUnread     => "prev_unread",         "Previous conversation with unread","navigate";

    CursorDown     => "cursor_down",     "Next message",                         "messages";
    CursorUp       => "cursor_up",       "Previous message",                     "messages";
    PageDown       => "page_down",       "Half a page down",                     "messages";
    PageUp         => "page_up",         "Half a page up",                       "messages";
    GotoNewest     => "goto_newest",     "Newest message, and follow",           "messages";
    GotoOldest     => "goto_oldest",     "Oldest loaded message",                "messages";
    OpenThread     => "open_thread",     "Open the thread",                      "messages";
    CloseThread    => "close_thread",    "Close the thread",                     "messages";
    MarkRead       => "mark_read",       "Mark the conversation read",           "messages";
    CopyText       => "copy_text",       "Copy the message text",                "messages";
    CopyLink       => "copy_link",       "Copy a link to the message",           "messages";
    OpenLink       => "open_link",       "Open the first link",                  "messages";
    DownloadFiles  => "download_files",  "Save this message's files",            "messages";
    ViewImage      => "view_image",      "Open the image at its own size",       "messages";
    UploadFile     => "upload_file",     "Send a file",                          "messages";
    React          => "react",           "Add a reaction",                       "messages";
    ReactQuick1    => "react_1",         "React with the first quick emoji",     "messages";
    ReactQuick2    => "react_2",         "React with the second",                "messages";
    ReactQuick3    => "react_3",         "React with the third",                 "messages";
    ReactQuick4    => "react_4",         "React with the fourth",                "messages";
    ReactQuick5    => "react_5",         "React with the fifth",                 "messages";
    Quote          => "quote",           "Quote this into the composer",         "messages";
    Forward        => "forward_message", "Forward this to another conversation", "messages";
    ViewSource     => "view_source",     "The JSON Slack sent for this",         "messages";
    EditMessage    => "edit_message",    "Edit your message",                    "messages";
    DeleteMessage  => "delete_message",  "Delete your message",                  "messages";

    Insert         => "insert",          "Start typing",                         "compose";
    Normal         => "normal",          "Stop typing",                          "compose";
    Send           => "send",            "Send the message",                     "compose";
    Newline        => "newline",         "Insert a line break",                  "compose";
    ClearComposer  => "clear_composer",  "Clear the composer",                   "compose";
    EditorEscape   => "editor",          "Edit the draft in $EDITOR",            "compose";
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
