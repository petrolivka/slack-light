//! Spike E: `slk-config`'s actions as GTK actions and accelerators.
//!
//! The keymap is data — chords to action names — and the window binds each
//! action name to a `gio::SimpleAction` with the chord as its accelerator.
//! That is what keeps remapping and a generated shortcuts window: change the
//! data, nothing else moves.

use crate::app::Msg;
use crate::logic::accel;
use gtk::prelude::*;
use slk_config::{Action, Keymap, Mode};

/// The subset the spike exercises, each with a default for when the
/// preset does not bind it — the non-modal preset leaves a few of these
/// to the config file.
const WANTED: [(Action, &str); 8] = [
    (Action::JumpTo, "<Control>k"),
    (Action::NextConv, "<Alt>Down"),
    (Action::PrevConv, "<Alt>Up"),
    (Action::Normal, "Escape"),
    (Action::Help, "F1"),
    (Action::Workspace1, "<Control>1"),
    (Action::ClearComposer, "<Control>u"),
    (Action::Palette, "<Control><Shift>p"),
];

/// What got bound: (action name, accelerator, came from the preset).
pub fn install(app: &gtk::Application, sender: relm4::Sender<Msg>) -> Vec<(String, String, bool)> {
    let keymap = Keymap::preset("slack");
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
