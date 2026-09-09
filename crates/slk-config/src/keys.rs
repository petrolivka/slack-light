//! Key bindings: parsing them, and looking them up.

use crate::action::Action;
use crossterm_stub::{KeyCode, KeyModifiers};
use std::collections::HashMap;
use std::str::FromStr;

/// The subset of crossterm's key model this crate needs, so `slk-config` does
/// not depend on a terminal library to describe a binding.
pub mod crossterm_stub {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum KeyCode {
        Char(char),
        Enter,
        Esc,
        Tab,
        BackTab,
        Backspace,
        Left,
        Right,
        Up,
        Down,
        Home,
        End,
        PageUp,
        PageDown,
        Delete,
        F(u8),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct KeyModifiers {
        pub ctrl: bool,
        pub alt: bool,
        pub shift: bool,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Chord {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Chord { code, mods }
    }
    pub fn plain(c: char) -> Self {
        Chord::new(KeyCode::Char(c), KeyModifiers::default())
    }
    pub fn ctrl(c: char) -> Self {
        Chord::new(
            KeyCode::Char(c),
            KeyModifiers {
                ctrl: true,
                ..Default::default()
            },
        )
    }
    pub fn key(code: KeyCode) -> Self {
        Chord::new(code, KeyModifiers::default())
    }
    pub fn alt(c: char) -> Self {
        Chord::new(
            KeyCode::Char(c),
            KeyModifiers {
                alt: true,
                ..Default::default()
            },
        )
    }

    /// Render a binding the way the help overlay shows it, and the way the
    /// config file writes it, so the two never disagree.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.mods.ctrl {
            out.push_str("ctrl+");
        }
        if self.mods.alt {
            out.push_str("alt+");
        }
        if self.mods.shift {
            out.push_str("shift+");
        }
        out.push_str(&match self.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "shift+tab".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::F(n) => format!("f{n}"),
        });
        out
    }
}

impl FromStr for Chord {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut mods = KeyModifiers::default();
        let lower = s.to_ascii_lowercase();
        let mut rest = lower.as_str();
        loop {
            if let Some(r) = rest.strip_prefix("ctrl+").or(rest.strip_prefix("c-")) {
                mods.ctrl = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("alt+").or(rest.strip_prefix("m-")) {
                mods.alt = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("shift+").or(rest.strip_prefix("s-")) {
                mods.shift = true;
                rest = r;
            } else {
                break;
            }
        }
        let code = match rest {
            "enter" | "return" | "cr" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "backspace" | "bs" => KeyCode::Backspace,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "delete" | "del" => KeyCode::Delete,
            "space" => KeyCode::Char(' '),
            other => {
                if let Some(n) = other.strip_prefix('f').and_then(|d| d.parse::<u8>().ok()) {
                    KeyCode::F(n)
                } else {
                    let mut cs = other.chars();
                    match (cs.next(), cs.next()) {
                        (Some(c), None) => KeyCode::Char(c),
                        _ => return Err(format!("cannot parse key {s:?}")),
                    }
                }
            }
        };
        Ok(Chord { code, mods })
    }
}

/// Which set of bindings applies right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Normal,
    Insert,
}

#[derive(Debug, Clone)]
pub struct Keymap {
    normal: HashMap<Chord, Action>,
    insert: HashMap<Chord, Action>,
    /// Two-key sequences, written `g g` in the config. Vim habits are mostly
    /// chords, and a client that has `G` but not `gg` is halfway to the
    /// bindings people already have in their fingers.
    chords: HashMap<(Chord, Chord), Action>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap::vim()
    }
}

impl Keymap {
    /// The default: modal, vim-shaped, with the composer as insert mode.
    pub fn vim() -> Self {
        use Action::*;
        use KeyCode::*;
        let mut normal = HashMap::new();
        let mut insert = HashMap::new();

        for (chord, action) in [
            (Chord::plain('q'), Quit),
            (Chord::ctrl('c'), Quit),
            (Chord::plain('?'), Help),
            (Chord::plain(':'), Palette),
            (Chord::ctrl('l'), Redraw),
            (Chord::ctrl('z'), Suspend),
            (Chord::plain('F'), FollowThread),
            (Chord::plain('C'), BrowseChannels),
            (Chord::plain('B'), ToggleBroadcast),
            // ctrl-o and ctrl-i are vim's, and vim gets away with ctrl-i
            // because it treats it as Tab. Here Tab cycles the panes, and in a
            // terminal without the kitty keyboard protocol the two arrive as
            // the same byte — so ctrl-i alone would be a binding that silently
            // does nothing. The brackets are the ones that always work.
            (Chord::ctrl('o'), GoBack),
            (Chord::ctrl('i'), GoForward),
            (Chord::plain('['), GoBack),
            (Chord::plain(']'), GoForward),
            (Chord::ctrl('b'), ToggleSidebar),
            (Chord::key(Tab), FocusNext),
            (Chord::key(BackTab), FocusPrev),
            (Chord::ctrl('k'), JumpTo),
            (Chord::ctrl('w'), NextWorkspace),
            (
                Chord::new(
                    Char('1'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace1,
            ),
            (
                Chord::new(
                    Char('2'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace2,
            ),
            (
                Chord::new(
                    Char('3'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace3,
            ),
            (
                Chord::new(
                    Char('4'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace4,
            ),
            (
                Chord::new(
                    Char('5'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace5,
            ),
            (Chord::plain('/'), Search),
            (Chord::plain('T'), Threads),
            (Chord::plain('S'), Saved),
            (Chord::plain('s'), Save),
            (Chord::plain('p'), Pin),
            (Chord::plain('@'), Mentions),
            (Chord::plain('m'), Members),
            (Chord::plain('u'), Profile),
            (Chord::ctrl('f'), SearchLocal),
            (Chord::plain('j'), CursorDown),
            (Chord::plain('k'), CursorUp),
            (Chord::key(Down), CursorDown),
            (Chord::key(Up), CursorUp),
            (Chord::ctrl('d'), Action::PageDown),
            (Chord::ctrl('u'), Action::PageUp),
            (Chord::plain('G'), GotoNewest),
            (Chord::key(Enter), OpenThread),
            (Chord::plain('t'), OpenThread),
            (Chord::key(Esc), CloseThread),
            (Chord::plain('M'), MarkRead),
            (Chord::plain('y'), CopyText),
            (Chord::plain('Y'), CopyLink),
            (Chord::plain('o'), OpenLink),
            (Chord::plain('f'), DownloadFiles),
            (Chord::ctrl('u'), UploadFile),
            (Chord::plain('r'), React),
            (Chord::plain('1'), ReactQuick1),
            (Chord::plain('2'), ReactQuick2),
            (Chord::plain('3'), ReactQuick3),
            (Chord::plain('4'), ReactQuick4),
            (Chord::plain('5'), ReactQuick5),
            (Chord::plain('e'), EditMessage),
            (Chord::plain('d'), DeleteMessage),
            (Chord::plain('i'), Insert),
            (Chord::plain('a'), Insert),
            (
                Chord::new(
                    Down,
                    KeyModifiers {
                        alt: true,
                        ..Default::default()
                    },
                ),
                NextConv,
            ),
            (
                Chord::new(
                    Up,
                    KeyModifiers {
                        alt: true,
                        ..Default::default()
                    },
                ),
                PrevConv,
            ),
            (Chord::plain('n'), NextUnread),
            (Chord::plain('N'), PrevUnread),
        ] {
            normal.insert(chord, action);
        }

        for (chord, action) in [
            (Chord::key(Esc), Normal),
            (Chord::key(Enter), Send),
            // M0 measured that kitty, foot, alacritty and ghostty all report
            // shift+enter distinctly, and that tmux does not. alt+enter and
            // ctrl+j are the portable fallbacks, so all three are bound.
            (
                Chord::new(
                    Enter,
                    KeyModifiers {
                        shift: true,
                        ..Default::default()
                    },
                ),
                Newline,
            ),
            (
                Chord::new(
                    Enter,
                    KeyModifiers {
                        alt: true,
                        ..Default::default()
                    },
                ),
                Newline,
            ),
            (Chord::ctrl('j'), Newline),
            (Chord::ctrl('u'), ClearComposer),
            (Chord::ctrl('x'), EditorEscape),
            (Chord::ctrl('k'), JumpTo),
            (Chord::ctrl('w'), NextWorkspace),
            (
                Chord::new(
                    Char('1'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace1,
            ),
            (
                Chord::new(
                    Char('2'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace2,
            ),
            (
                Chord::new(
                    Char('3'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace3,
            ),
            (
                Chord::new(
                    Char('4'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace4,
            ),
            (
                Chord::new(
                    Char('5'),
                    KeyModifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                Workspace5,
            ),
            (Chord::ctrl('c'), Quit),
        ] {
            insert.insert(chord, action);
        }

        let mut chords = HashMap::new();
        chords.insert((Chord::plain('g'), Chord::plain('g')), GotoOldest);
        chords.insert((Chord::plain('z'), Chord::plain('z')), Redraw);
        // vim's own fold toggle, and `z` is already a prefix here so a bare
        // `z` could not have it without breaking `zz`.
        chords.insert((Chord::plain('z'), Chord::plain('a')), ToggleSection);

        Keymap {
            normal,
            insert,
            chords,
        }
    }

    /// The keys, with the composer always focused and navigation on chords.
    ///
    /// For people who do not want modes. Everything reachable in the modal
    /// keymap is reachable here; it just costs a modifier, because the plain
    /// letters are text.
    pub fn non_modal() -> Self {
        use Action::*;
        use KeyCode::*;
        let alt = |c: char| {
            Chord::new(
                Char(c),
                KeyModifiers {
                    alt: true,
                    ..Default::default()
                },
            )
        };
        let alt_key = |k: KeyCode| {
            Chord::new(
                k,
                KeyModifiers {
                    alt: true,
                    ..Default::default()
                },
            )
        };

        let mut insert = HashMap::new();
        for (chord, action) in [
            (Chord::key(Enter), Send),
            (
                Chord::new(
                    Enter,
                    KeyModifiers {
                        shift: true,
                        ..Default::default()
                    },
                ),
                Newline,
            ),
            (
                Chord::new(
                    Enter,
                    KeyModifiers {
                        alt: true,
                        ..Default::default()
                    },
                ),
                Newline,
            ),
            (Chord::ctrl('j'), Newline),
            (Chord::ctrl('c'), Quit),
            (Chord::ctrl('z'), Suspend),
            // ctrl-o and ctrl-i are vim's, and vim gets away with ctrl-i
            // because it treats it as Tab. Here Tab cycles the panes, and in a
            // terminal without the kitty keyboard protocol the two arrive as
            // the same byte — so ctrl-i alone would be a binding that silently
            // does nothing. The brackets are the ones that always work.
            (Chord::ctrl('o'), GoBack),
            (Chord::ctrl('i'), GoForward),
            (Chord::plain('['), GoBack),
            (Chord::plain(']'), GoForward),
            (Chord::ctrl('k'), JumpTo),
            (Chord::ctrl('w'), NextWorkspace),
            (Chord::ctrl('x'), EditorEscape),
            (Chord::ctrl('u'), ClearComposer),
            (Chord::key(F(1)), Help),
            (Chord::ctrl('p'), Palette),
            (Chord::ctrl('f'), SearchLocal),
            (alt('/'), Search),
            (alt_key(Up), CursorUp),
            (alt_key(Down), CursorDown),
            (alt('k'), CursorUp),
            (alt('j'), CursorDown),
            (alt('g'), GotoNewest),
            (alt('t'), OpenThread),
            (alt('F'), FollowThread),
            (alt('c'), BrowseChannels),
            (alt('z'), ToggleSection),
            (alt('['), GoBack),
            (alt(']'), GoForward),
            (alt('b'), ToggleBroadcast),
            (Chord::key(Esc), CloseThread),
            (alt('r'), React),
            (alt('e'), EditMessage),
            (alt('d'), DeleteMessage),
            (alt('y'), CopyText),
            (alt('o'), OpenLink),
            (alt('s'), Save),
            (alt('p'), Pin),
            (alt('f'), DownloadFiles),
            (alt('u'), UploadFile),
            (alt('m'), Members),
            (alt('i'), Profile),
            (alt('@'), Mentions),
            (alt('T'), Threads),
            (alt('S'), Saved),
            (alt('b'), ToggleSidebar),
            (Chord::key(Tab), FocusNext),
            (alt('M'), MarkRead),
        ] {
            insert.insert(chord, action);
        }

        // Normal mode still exists — Esc reaches it — so nothing is lost for
        // someone who wants it occasionally.
        let modal = Keymap::vim();
        Keymap {
            normal: modal.normal,
            insert,
            chords: HashMap::new(),
        }
    }

    /// Which mode a preset starts in.
    ///
    /// The non-modal one starts in the composer, because that is the whole
    /// point of it: plain letters are text, and having to press a key first
    /// would make it modal with extra steps.
    pub fn start_mode(name: &str) -> Mode {
        match name {
            "slack" | "non-modal" | "nonmodal" => Mode::Insert,
            _ => Mode::Normal,
        }
    }

    /// The preset a name asks for.
    pub fn preset(name: &str) -> Self {
        match name {
            "slack" | "non-modal" | "nonmodal" => Keymap::non_modal(),
            _ => Keymap::vim(),
        }
    }

    pub fn get(&self, mode: Mode, chord: &Chord) -> Option<Action> {
        match mode {
            Mode::Normal => self.normal.get(chord).copied(),
            Mode::Insert => self.insert.get(chord).copied(),
        }
    }

    /// Is this key the start of a two-key sequence?
    pub fn starts_chord(&self, mode: Mode, chord: &Chord) -> bool {
        mode == Mode::Normal && self.chords.keys().any(|(first, _)| first == chord)
    }

    pub fn chord(&self, first: &Chord, second: &Chord) -> Option<Action> {
        self.chords.get(&(*first, *second)).copied()
    }

    /// Apply user overrides. A binding that does not parse is reported and
    /// skipped, never fatal: one typo must not cost the user their whole
    /// keymap.
    ///
    /// A key written with a space in it — `"g g"` — is a two-key sequence.
    pub fn apply(&mut self, mode: Mode, table: &HashMap<String, String>) -> Vec<String> {
        let mut problems = Vec::new();
        for (k, v) in table {
            let action = match Action::from_str(v) {
                Ok(a) => a,
                Err(e) => {
                    problems.push(e);
                    continue;
                }
            };
            let keys: Vec<&str> = k.split_whitespace().collect();
            let parsed: Result<Vec<Chord>, String> =
                keys.iter().map(|k| Chord::from_str(k)).collect();
            match parsed.as_deref() {
                Ok([one]) => {
                    match mode {
                        Mode::Normal => self.normal.insert(*one, action),
                        Mode::Insert => self.insert.insert(*one, action),
                    };
                }
                Ok([first, second]) => {
                    self.chords.insert((*first, *second), action);
                }
                Ok(_) => problems.push(format!("{k:?}: only one or two keys, not more")),
                Err(e) => problems.push(e.clone()),
            }
        }
        problems
    }

    /// Every binding for an action, for the help overlay.
    pub fn bindings_for(&self, mode: Mode, action: Action) -> Vec<String> {
        let map = match mode {
            Mode::Normal => &self.normal,
            Mode::Insert => &self.insert,
        };
        let mut v: Vec<String> = map
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(c, _)| c.render())
            .collect();
        if mode == Mode::Normal {
            v.extend(
                self.chords
                    .iter()
                    .filter(|(_, a)| **a == action)
                    .map(|((f, s), _)| format!("{}{}", f.render(), s.render())),
            );
        }
        v.sort();
        v
    }
}
