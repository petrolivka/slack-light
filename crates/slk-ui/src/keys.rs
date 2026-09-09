//! `slk-config`'s actions as GTK actions and accelerators.
//!
//! The keymap is data — chords to action names — and the window binds each
//! action name to a `gio::SimpleAction` with the chord as its accelerator.
//! That is what keeps remapping and a generated shortcuts window: change the
//! data, nothing else moves.
//!
//! Two rules decide what actually gets an accelerator, and both were learned
//! the hard way when the whole action list was first turned on:
//!
//! 1. **A chord with no modifier is text, not a shortcut.** The `slack`
//!    preset keeps the composer focused, and it binds `[` and `]` to back and
//!    forward. Installed as accelerators those swallow every bracket typed
//!    into a code snippet. `logic::bindable` is the gate.
//! 2. **The first action to claim an accelerator keeps it.** The preset binds
//!    Escape to `close_thread` and the defaults bind it to `normal`; GTK will
//!    happily accept both and then fire whichever it feels like. One key, one
//!    action, and the loser is reported rather than silently dropped.

use crate::app::Msg;
use crate::logic::{accel, bindable};
use gtk::prelude::*;
use slk_config::{Action, Keymap, Mode};
use std::collections::HashSet;

/// Actions the window does not answer to as an accelerator.
///
/// Send and Newline belong to the composer's own key controller — an
/// accelerator on Return would fire before the TextView ever saw it. Quit on
/// ctrl-c would take the window down instead of copying. The rest are
/// terminal-era actions with no meaning in a window.
const NOT_ACCELERATORS: &[Action] = &[
    Action::Send,
    Action::Newline,
    Action::Quit,
    Action::Suspend,
    Action::Redraw,
    Action::Insert,
];

/// A default for an action the preset leaves unbound, or binds to something
/// that cannot be an accelerator. Escape is deliberately `normal`, which
/// unwinds whatever is open — see `App::escape`.
const DEFAULTS: &[(Action, &str)] = &[
    (Action::JumpTo, "<Control>k"),
    (Action::NextConv, "<Alt>Down"),
    (Action::PrevConv, "<Alt>Up"),
    (Action::NextUnread, "<Alt><Shift>Down"),
    (Action::PrevUnread, "<Alt><Shift>Up"),
    (Action::Normal, "Escape"),
    (Action::Help, "F1"),
    (Action::Palette, "<Control><Alt>p"),
    (Action::ClearComposer, "<Control>u"),
    (Action::NextWorkspace, "<Control>w"),
    (Action::PrevWorkspace, "<Control><Alt>w"),
    (Action::Workspace1, "<Control>1"),
    (Action::Workspace2, "<Control>2"),
    (Action::Workspace3, "<Control>3"),
    (Action::Workspace4, "<Control>4"),
    (Action::Workspace5, "<Control>5"),
    (Action::GoBack, "<Alt>bracketleft"),
    (Action::GoForward, "<Alt>bracketright"),
    (Action::CursorUp, "<Alt>k"),
    (Action::CursorDown, "<Alt>j"),
    (Action::PageUp, "<Alt>Page_Up"),
    (Action::PageDown, "<Alt>Page_Down"),
    (Action::GotoNewest, "<Alt>g"),
    (Action::GotoOldest, "<Alt>Home"),
    (Action::ReactQuick1, "<Alt>1"),
    (Action::ReactQuick2, "<Alt>2"),
    (Action::ReactQuick3, "<Alt>3"),
    (Action::ReactQuick4, "<Alt>4"),
    (Action::ReactQuick5, "<Alt>5"),
    (Action::CloseThread, "<Alt>w"),
    (Action::ViewImage, "<Alt>v"),
    (Action::ToggleSidebar, "<Control><Alt>b"),
    // The preset asks for these on alt+SHIFT+letter, which never arrives —
    // see `logic::bindable`. Control chords do, so that is what gets
    // installed, and the shortcuts window shows what is real rather than
    // what was wished for.
    (Action::Threads, "<Control>t"),
    (Action::Saved, "<Control>d"),
    (Action::MarkRead, "<Control>r"),
    (Action::FollowThread, "<Control>n"),
    (Action::ToggleBroadcast, "<Control>b"),
    (Action::CopyLink, "<Control>l"),
    (Action::Star, "<Control><Alt>s"),
    (Action::Mute, "<Control><Alt>m"),
    // Not <Control><Alt>p, which reads better and is already this table's
    // default for the palette. Two entries wanting one chord is a latent
    // collision that only shows up under a preset that leaves the other one
    // unbound, and the test below refuses it.
    (Action::Pinned, "<Control><Alt>i"),
    // Topic, purpose, invite and leave are deliberately keyless: they are
    // rare, they are not reversible by pressing the same key again, and a
    // mis-typed chord that leaves a channel is not a mistake anyone forgives.
    // The palette and the conversation menu reach them by name.
];

/// One installed binding, for the metrics log and the shortcuts window.
pub struct Binding {
    pub action: Action,
    pub accel: String,
    /// `preset`, `default`, or why it has none.
    pub source: &'static str,
}

/// Bind every action the window answers to. Returns what got bound, in the
/// action list's own order, so the shortcuts window is grouped and stable.
pub fn install(app: &gtk::Application, preset: &str, sender: relm4::Sender<Msg>) -> Vec<Binding> {
    let keymap = Keymap::preset(preset);
    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::new();

    // Two passes: everything the preset asked for first, so a default can
    // never take an accelerator out from under a binding the user chose.
    let mut chosen: Vec<(Action, Option<(String, &'static str)>)> = Vec::new();
    for &action in Action::ALL {
        if NOT_ACCELERATORS.contains(&action) {
            continue;
        }
        let from_preset = keymap
            .bindings_for(Mode::Insert, action)
            .into_iter()
            .filter(|r| bindable(r))
            .find_map(|r| accel(&r))
            .filter(|a| !taken.contains(a));
        if let Some(a) = &from_preset {
            taken.insert(a.clone());
        }
        chosen.push((action, from_preset.map(|a| (a, "preset"))));
    }
    for (action, slot) in chosen.iter_mut() {
        if slot.is_some() {
            continue;
        }
        if let Some((_, d)) = DEFAULTS.iter().find(|(a, _)| a == action) {
            if taken.insert(d.to_string()) {
                *slot = Some((d.to_string(), "default"));
            }
        }
    }

    for (action, slot) in chosen {
        let name = action.name().to_string();
        // The action exists whether or not it has a key: the command palette
        // runs it by name, and so does the test harness.
        let simple = gtk::gio::SimpleAction::new(&name, None);
        let s = sender.clone();
        let n = name.clone();
        simple.connect_activate(move |_, _| {
            let _ = s.send(Msg::Action(n.clone()));
        });
        app.add_action(&simple);
        match slot {
            Some((acc, source)) => {
                app.set_accels_for_action(&format!("app.{name}"), &[acc.as_str()]);
                out.push(Binding {
                    action,
                    accel: acc,
                    source,
                });
            }
            None => out.push(Binding {
                action,
                accel: String::new(),
                source: "no free key",
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::DEFAULTS;
    use std::collections::HashMap;

    /// No two defaults may ask for the same chord.
    ///
    /// The first-claimant rule makes a collision survivable rather than
    /// silent, but which of the two wins then depends on the preset: under
    /// one keymap the pinned list has a key and under another it does not,
    /// and nothing says why. Within this table it is simply a mistake.
    #[test]
    fn no_two_defaults_want_the_same_chord() {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for (action, accel) in DEFAULTS {
            if let Some(other) = seen.insert(accel, action.name()) {
                panic!(
                    "{accel} is the default for both {other} and {}",
                    action.name()
                );
            }
        }
    }
}
