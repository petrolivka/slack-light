//! `slk-config`'s actions as GTK actions and accelerators.
//!
//! The keymap is data — chords to action names — and the window binds each
//! action name to a `gio::SimpleAction` with the chord as its accelerator.
//! That is what keeps remapping and a generated shortcuts window: change the
//! data, nothing else moves.

use crate::app::Msg;
use crate::logic::accel;
use gtk::prelude::*;
use slk_config::{Action, Keymap, Mode};

/// The actions the window answers to, each with a default for when the
/// preset does not bind it.
const WANTED: [(Action, &str); 13] = [
    (Action::JumpTo, "<Control>k"),
    (Action::NextConv, "<Alt>Down"),
    (Action::PrevConv, "<Alt>Up"),
    (Action::Normal, "Escape"),
    (Action::Help, "F1"),
    (Action::Palette, "<Control><Shift>p"),
    (Action::ClearComposer, "<Control>u"),
    (Action::NextWorkspace, "<Control>w"),
    (Action::Workspace1, "<Control>1"),
    (Action::Workspace2, "<Control>2"),
    (Action::Workspace3, "<Control>3"),
    (Action::Workspace4, "<Control>4"),
    (Action::Workspace5, "<Control>5"),
];

/// What got bound: (action name, accelerator, came from the preset).
pub fn install(
    app: &gtk::Application,
    preset: &str,
    sender: relm4::Sender<Msg>,
) -> Vec<(String, String, bool)> {
    let keymap = Keymap::preset(preset);
    let mut out = Vec::new();
    for (action, default) in WANTED {
        let from_preset = keymap
            .bindings_for(Mode::Insert, action)
            .into_iter()
            .find_map(|r| accel(&r));
        let (acc, from) = match from_preset {
            Some(a) => (a, true),
            None => (default.to_string(), false),
        };
        let name = action.name().to_string();
        let simple = gtk::gio::SimpleAction::new(&name, None);
        let s = sender.clone();
        let n = name.clone();
        simple.connect_activate(move |_, _| {
            let _ = s.send(Msg::Action(n.clone()));
        });
        app.add_action(&simple);
        app.set_accels_for_action(&format!("app.{name}"), &[acc.as_str()]);
        out.push((name, acc, from));
    }
    out
}
