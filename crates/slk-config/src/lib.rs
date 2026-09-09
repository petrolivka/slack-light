//! Configuration, and the keymap it can rewrite.

pub mod action;
pub mod keys;

pub use action::Action;
pub use keys::{Chord, Keymap, Mode};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub sidebar: Sidebar,
    pub message: MessageCfg,
    pub composer: Composer,
    pub emoji: Emoji,
    pub images: ImagesCfg,
    pub files: Files,
    pub notify: Notify,
    pub keymap: KeymapCfg,
    pub ui: Ui,
    pub theme: ThemeCfg,
    pub window: WindowCfg,
    pub store: StoreCfg,
    pub log: Log,
    /// `[keys.normal]` and `[keys.insert]`: chord to action name.
    pub keys: Keys,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub default_workspace: String,
    pub confirm_quit: bool,
}
impl Default for General {
    fn default() -> Self {
        General {
            default_workspace: String::new(),
            confirm_quit: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Sidebar {
    pub width: u16,
    pub unread_first: bool,
    pub hide_read: bool,
}
impl Default for Sidebar {
    fn default() -> Self {
        Sidebar {
            width: 22,
            unread_first: false,
            hide_read: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageCfg {
    /// `absolute` shows a clock time, `relative` shows "5m".
    pub timestamp: String,
    /// Merge consecutive messages from one author within five minutes.
    pub group: bool,
    pub hide_joins: bool,
    /// `on_focus`, `on_view` or `manual`. See the requirements, §7.2: getting
    /// this wrong means marking things read the user never saw.
    pub mark_read: String,
    pub history_page: u16,
}
impl Default for MessageCfg {
    fn default() -> Self {
        MessageCfg {
            timestamp: "absolute".into(),
            group: true,
            hide_joins: false,
            mark_read: "on_focus".into(),
            history_page: 50,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Composer {
    /// `enter` or `ctrl-enter`.
    pub send: String,
    /// Overrides `$VISUAL` and `$EDITOR`.
    pub editor: String,
}
impl Default for Composer {
    fn default() -> Self {
        Composer {
            send: "enter".into(),
            editor: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Emoji {
    /// `unicode` or `shortcode`. The escape hatch for terminals whose emoji
    /// widths cannot be made to line up.
    pub render: String,
    /// Slack's skin tone, 2 to 6, or 0 for the yellow default. Applied to what
    /// the picker offers and to the reaction that is sent, so the two agree.
    pub skin_tone: u8,
}
impl Default for Emoji {
    fn default() -> Self {
        Emoji {
            render: "unicode".into(),
            skin_tone: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ImagesCfg {
    pub enabled: bool,
    /// `auto`, `kitty`, `sixel`, `iterm2`, `halfblock` or `off`. The escape
    /// hatch for a terminal that answers a capability query wrongly, and for a
    /// multiplexer that hides the one underneath.
    pub backend: String,
    pub max_width: u16,
    pub max_height: u16,
    /// How much disk the fetched pictures may hold. Bounded because a cache
    /// that only grows is a disk that fills, and these are copies of things
    /// Slack still has.
    pub cache_mb: u64,
}
impl Default for ImagesCfg {
    fn default() -> Self {
        ImagesCfg {
            cache_mb: 256,
            enabled: true,
            backend: "auto".into(),
            max_width: 60,
            max_height: 12,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Files {
    /// Where `f` saves what it downloads. `~` is expanded.
    pub download_dir: String,
}
impl Default for Files {
    fn default() -> Self {
        Files {
            download_dir: "~/Downloads".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Notify {
    /// A desktop notification. Needs a session bus, so it is useless over SSH.
    pub desktop: bool,
    /// The terminal bell. Reaches the far end of an SSH link, and is easy to
    /// miss.
    pub bell: bool,
    /// `auto`, `osc9`, `osc777` or `off`. Asks the terminal emulator itself to
    /// raise the notification, which is the channel that survives SSH.
    pub osc: String,
    /// Words that notify wherever they appear, the way Slack's own highlight
    /// words do: a project name, a service you are on call for, your surname.
    /// Matched whole and without regard to case.
    pub keywords: Vec<String>,
}
impl Default for Notify {
    fn default() -> Self {
        Notify {
            desktop: true,
            bell: false,
            keywords: Vec::new(),
            osc: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapCfg {
    /// `vim` for modal keys, `slack` for a composer that is always focused and
    /// navigation on modifiers.
    pub preset: String,
}
impl Default for KeymapCfg {
    fn default() -> Self {
        KeymapCfg {
            // Non-modal: a window whose composer swallows `j` because it is
            // in the wrong mode is a bug report. `vim` is one line away.
            preset: "slack".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub theme: String,
    /// Draw with nothing above U+007F: no box drawing, no pictographs.
    pub ascii: bool,
    /// Stop anything from changing on its own. Typing indicators disappear and
    /// notices stay until something replaces them, so what was read a moment
    /// ago is still there to be read again.
    pub reduced_motion: bool,
    /// One column, top to bottom, no panes and no chrome. What a screen reader
    /// can follow, and what a braille display can hold.
    pub linear: bool,
    pub mouse: bool,
}
impl Default for Ui {
    fn default() -> Self {
        Ui {
            theme: "slack-dark".into(),
            ascii: false,
            reduced_motion: false,
            linear: false,
            mouse: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeCfg {
    /// `auto` uses Omarchy's palette when its state directory exists, else a
    /// built-in; `omarchy`, `builtin` and `file` insist.
    pub source: String,
    /// `auto` follows the desktop's colour scheme; `dark` or `light` insist.
    pub builtin: String,
    /// A `colors.toml` of your own, in Omarchy's schema.
    pub file: String,
    /// Layer `~/.config/slack-light/user.css` on top of the generated CSS.
    pub user_css: bool,
}
impl Default for ThemeCfg {
    fn default() -> Self {
        ThemeCfg {
            source: "auto".into(),
            builtin: "auto".into(),
            file: String::new(),
            user_css: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowCfg {
    pub width: i32,
    pub height: i32,
    pub sidebar_width: i32,
}
impl Default for WindowCfg {
    fn default() -> Self {
        WindowCfg {
            width: 1100,
            height: 720,
            sidebar_width: 240,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreCfg {
    pub enabled: bool,
    pub max_messages_per_channel: u32,
    pub max_age_days: u32,
}
impl Default for StoreCfg {
    fn default() -> Self {
        StoreCfg {
            enabled: true,
            max_messages_per_channel: 5000,
            max_age_days: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Log {
    pub level: String,
    pub file: bool,
}
impl Default for Log {
    fn default() -> Self {
        Log {
            level: "info".into(),
            file: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Keys {
    pub normal: HashMap<String, String>,
    pub insert: HashMap<String, String>,
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// Load, or return the defaults when there is no file. A *broken* file is a
    /// different matter: it is reported rather than silently ignored, because a
    /// config that quietly does nothing is worse than one that complains.
    pub fn load() -> (Self, Vec<String>) {
        let p = Self::path();
        let Ok(text) = std::fs::read_to_string(&p) else {
            return (Config::default(), Vec::new());
        };
        match toml::from_str::<Config>(&text) {
            Ok(c) => (c, Vec::new()),
            Err(e) => (
                Config::default(),
                vec![format!("{}: {e}; using defaults", p.display())],
            ),
        }
    }

    /// Build the keymap this config asks for, returning any bad bindings so the
    /// caller can show them.
    pub fn keymap(&self) -> (Keymap, Vec<String>) {
        let mut km = Keymap::preset(&self.keymap.preset);
        let mut problems = km.apply(Mode::Normal, &self.keys.normal);
        problems.extend(km.apply(Mode::Insert, &self.keys.insert));
        (km, problems)
    }

    /// The colours a named theme file sets, if there is one.
    ///
    /// `themes/<name>.toml` beside the config, a flat table of slot names to
    /// colours. Nothing here validates them; the renderer owns what a slot is.
    pub fn theme_overrides(name: &str) -> Vec<(String, String)> {
        let p = config_dir().join("themes").join(format!("{name}.toml"));
        let Ok(text) = std::fs::read_to_string(p) else {
            return Vec::new();
        };
        toml::from_str::<HashMap<String, String>>(&text)
            .map(|m| m.into_iter().collect())
            .unwrap_or_default()
    }

    pub fn write_default() -> Result<PathBuf> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let body =
            toml::to_string_pretty(&Config::default()).context("serialising the default config")?;
        std::fs::write(&p, format!("{HEADER}{body}"))?;
        Ok(p)
    }
}

const HEADER: &str = "\
# slack-light configuration.
#
# Every key maps to a named action; `slack-light --list-actions` prints them all,
# and the help overlay is generated from whatever is in force, so it cannot
# drift from what is written here.
#
#   [keys.normal]
#   \"ctrl+n\" = \"next_conversation\"
#
# The three under [ui] worth knowing about: `ascii` draws with nothing above
# U+007F, `reduced_motion` stops anything changing on its own, and `linear`
# gives one column top to bottom for a screen reader. Each has a command-line
# flag of the same name, for trying one without editing this file.
#
";

pub fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".config"))
        .join("slack-light")
}

pub fn data_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share"))
        .join("slack-light")
}

pub fn cache_dir() -> PathBuf {
    std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".cache"))
        .join("slack-light")
}

pub fn state_dir() -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/state"))
        .join("slack-light")
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}
