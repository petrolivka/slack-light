//! The window coloured by the desktop's theme, live.
//!
//! Omarchy does not theme GTK. What it does is keep `colors.toml` for the
//! current theme under `~/.local/state/omarchy/current/theme/` — a
//! *directory*, not a symlink: `omarchy-theme-set` builds the next theme
//! beside it, `rm -rf`s the old one and `mv`s the new one into place — and
//! then runs hooks. So the application reads the palette, writes its own
//! CSS, and watches the parent directory for the swap.
//!
//! Off Omarchy the same generator runs over a built-in palette in the same
//! schema, chosen by the desktop's `color-scheme`. One code path.

use gtk::prelude::*;
use std::path::{Path, PathBuf};

/// Omarchy's `colors.toml`, verbatim. Every key the file has today; the
/// derived ones templates get (`*_rgb`, `*_strip`, `selection_foreground`)
/// are computed, not stored.
#[derive(Debug, Clone, serde::Deserialize)]
#[allow(dead_code)] // the whole schema is kept so `--doctor` can print it; the CSS uses part of it
pub struct Palette {
    #[serde(default = "dark")]
    pub mode: String,
    pub accent: String,
    pub selection: String,
    pub muted: String,
    pub background: String,
    pub dark_background: String,
    pub darker_background: String,
    pub lighter_background: String,
    pub foreground: String,
    pub dark_foreground: String,
    pub light_foreground: String,
    pub bright_foreground: String,
    pub red: String,
    pub yellow: String,
    pub orange: String,
    pub green: String,
    pub cyan: String,
    pub blue: String,
    pub magenta: String,
    #[serde(default = "brown")]
    pub brown: String,
    pub bright_red: String,
    pub bright_yellow: String,
    pub bright_green: String,
    pub bright_cyan: String,
    pub bright_blue: String,
    pub bright_magenta: String,

    /// Not in `colors.toml`: how message bodies are set. The client fills
    /// these in from its own configuration after loading a palette.
    #[serde(skip, default = "mono_family")]
    pub mono_family: String,
    #[serde(skip, default = "mono_size")]
    pub mono_size: u8,
}

fn mono_family() -> String {
    "monospace".into()
}
fn mono_size() -> u8 {
    13
}

fn dark() -> String {
    "dark".into()
}
fn brown() -> String {
    "#75493d".into()
}

/// Two palettes of our own, in Omarchy's schema, for a desktop without it.
const BUILTIN_DARK: &str = include_str!("../themes/dark.toml");
const BUILTIN_LIGHT: &str = include_str!("../themes/light.toml");

pub fn omarchy_state_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".local/state/omarchy/current")
}

/// Where the palette came from, for `--doctor` and the findings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Omarchy(PathBuf),
    File(PathBuf),
    Builtin(&'static str),
}

/// How the configuration asks for the palette. Mirrors `[theme]` in
/// `config.toml` without depending on the config crate.
#[derive(Debug, Clone, Default)]
pub struct Choice {
    /// `auto` | `omarchy` | `builtin` | `file`
    pub source: String,
    /// `auto` | `dark` | `light`
    pub builtin: String,
    pub file: Option<PathBuf>,
}

pub fn load(choice: &Choice) -> (Palette, Source) {
    let source = choice.source.as_str();
    if source == "file" || source == "auto" {
        if let Some(p) = &choice.file {
            if let Some(pal) = read(p) {
                return (pal, Source::File(p.clone()));
            }
        }
    }
    if source == "omarchy" || source == "auto" {
        let omarchy = omarchy_state_dir().join("theme/colors.toml");
        if let Some(pal) = read(&omarchy) {
            return (pal, Source::Omarchy(omarchy));
        }
    }
    // Off Omarchy, or told to: the built-ins, chosen by the configuration
    // or, on `auto`, by the desktop's preference.
    let dark = match choice.builtin.as_str() {
        "dark" => true,
        "light" => false,
        _ => prefers_dark(),
    };
    if dark {
        (
            toml::from_str(BUILTIN_DARK).expect("built-in dark palette"),
            Source::Builtin("dark"),
        )
    } else {
        (
            toml::from_str(BUILTIN_LIGHT).expect("built-in light palette"),
            Source::Builtin("light"),
        )
    }
}

fn read(p: &Path) -> Option<Palette> {
    let text = std::fs::read_to_string(p).ok()?;
    match toml::from_str::<Palette>(&text) {
        Ok(pal) => Some(pal),
        Err(e) => {
            tracing::warn!("{}: {e}", p.display());
            None
        }
    }
}

/// The settings portal's `color-scheme`, which every desktop with a portal
/// answers; `gsettings` needs the GNOME schema installed and a minimal
/// Wayland box may not have it. 1 is "prefer dark".
fn prefers_dark() -> bool {
    let Ok(bus) =
        gtk::gio::bus_get_sync(gtk::gio::BusType::Session, None::<&gtk::gio::Cancellable>)
    else {
        return true;
    };
    let reply = bus.call_sync(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
        "Read",
        Some(&("org.freedesktop.appearance", "color-scheme").to_variant()),
        None,
        gtk::gio::DBusCallFlags::NONE,
        1000,
        None::<&gtk::gio::Cancellable>,
    );
    match reply {
        Ok(v) => {
            // (v) wrapping a variant wrapping u32.
            let inner = v.child_value(0);
            let inner = inner.as_variant().unwrap_or(inner);
            let inner = inner.as_variant().unwrap_or(inner);
            inner.get::<u32>() == Some(1)
        }
        Err(_) => true,
    }
}

/// The semantic colours the message renderer needs, as Pango hex. This is
/// the contract between the palette and the rows: whatever the source, the
/// rows only ever see these.
#[derive(Debug, Clone)]
pub struct Semantic {
    pub link: String,
    /// What to write *on* a filled highlight — the window's own background,
    /// which is the only colour guaranteed to read against an accent.
    pub on_fill: String,
    pub mention: String,
    pub mention_self_bg: String,
    pub dim: String,
    pub code_bg: String,
    pub authors: [String; 8],
}

impl Palette {
    pub fn semantic(&self) -> Semantic {
        Semantic {
            link: self.blue.clone(),
            on_fill: self.background.clone(),
            mention: self.accent.clone(),
            mention_self_bg: self.yellow.clone(),
            dim: self.dark_foreground.clone(),
            code_bg: self.muted.clone(),
            authors: [
                self.red.clone(),
                self.green.clone(),
                self.magenta.clone(),
                self.cyan.clone(),
                self.orange.clone(),
                self.bright_blue.clone(),
                self.bright_magenta.clone(),
                self.bright_cyan.clone(),
            ],
        }
    }

    /// The application CSS, generated. Named GTK colours (`@define-color`)
    /// so `user.css` can refer to them by name too.
    pub fn css(&self) -> String {
        let p = self;
        // Eight author colours as classes, because a per-row colour cannot
        // come from a stylesheet any other way and a provider per row would
        // cost more than the row.
        let authors: String = self
            .semantic()
            .authors
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(".avatar-{i} {{ background-color: {c}; }}\n.name-{i} {{ color: {c}; }}\n")
            })
            .collect();
        format!(
            r#"
@define-color sl_bg {bg};
@define-color sl_bg_dark {bgd};
@define-color sl_bg_darker {bgdd};
@define-color sl_bg_light {bgl};
@define-color sl_fg {fg};
@define-color sl_fg_dim {fgd};
@define-color sl_fg_bright {fgb};
@define-color sl_accent {acc};
@define-color sl_selection {sel};
@define-color sl_muted {mut};

/* Dense, dark, and quiet — the language of an editor rather than a
   dashboard. Separation by background and whitespace, never by a line. */
window {{ background-color: @sl_bg; color: @sl_fg; font-size: 13px; }}

/* The workspace rail: an activity bar. One square per workspace, the
   current one marked by an accent edge rather than a fill. */
.rail {{ background-color: @sl_bg_darker; }}
.rail button {{
    background: none; border: none; box-shadow: none; padding: 0;
    margin: 4px 6px; min-width: 32px; min-height: 32px;
    border-left: 2px solid transparent; border-radius: 0;
}}
.rail button:hover {{ background: alpha(@sl_fg, 0.06); }}
.rail button.current {{ border-left-color: @sl_accent; }}
.rail .tile {{
    min-width: 26px; min-height: 26px; border-radius: 6px;
    font-weight: bold; font-size: 12px; color: @sl_bg;
}}

.sidebar {{ background-color: @sl_bg_dark; }}
.sidebar list {{ background: none; }}
.sidebar row {{ padding: 0; min-height: 0; }}
.sidebar row:selected, .sidebar row:selected:hover {{ background: none; }}
.sidebar .conv {{
    padding: 3px 10px 3px 8px; border-radius: 5px; margin: 0 6px;
    color: @sl_fg;
}}
.sidebar row:hover .conv {{ background-color: alpha(@sl_fg, 0.05); }}
.sidebar row:selected .conv {{ background-color: @sl_selection; color: @sl_fg_bright; }}
.sidebar .unread {{ font-weight: bold; color: @sl_fg_bright; }}
.sidebar .section {{
    color: @sl_fg_dim; font-size: 11px; font-weight: bold;
    padding: 10px 8px 3px 8px; letter-spacing: 0.5px;
}}
.sidebar .section:hover {{ color: @sl_fg; }}
.badge {{
    background-color: {red}; color: @sl_bg; border-radius: 8px;
    padding: 0 5px; font-size: 10px; font-weight: bold;
}}
.count {{ color: @sl_fg_dim; font-size: 11px; }}

/* The conversation. No line between messages: grouping and whitespace do
   the separating, the way every chat client that reads well does it. */
.header {{ background-color: @sl_bg; padding: 6px 12px; }}
.header .title {{ color: @sl_fg_bright; font-weight: bold; }}
.header .topic {{ color: @sl_fg_dim; font-size: 12px; }}
.hairline {{ background-color: alpha(@sl_muted, 0.5); min-height: 1px; }}

.conversation {{ background-color: @sl_bg; }}
.conversation > row {{ padding: 0; }}
.conversation > row:hover {{ background-color: alpha(@sl_fg, 0.03); }}
/* The message cursor. A left edge rather than a filled row: a selection
   that repaints the whole message hides the grouping it sits inside. */
.conversation > row:selected {{
    background-color: alpha(@sl_accent, 0.10);
    box-shadow: inset 2px 0 0 0 @sl_accent;
}}
.avatar {{
    min-width: 24px; min-height: 24px; border-radius: 5px;
    color: @sl_bg; font-weight: bold; font-size: 11px;
}}
.body {{ font-family: {mono}; font-size: {mono_size}px; color: @sl_fg; }}
.time {{ color: @sl_fg_dim; font-family: monospace; font-size: 11px; }}
.meta {{ color: @sl_fg_dim; font-size: 11px; }}
.daybreak {{ color: @sl_fg_dim; font-size: 11px; font-weight: bold; letter-spacing: 0.5px; }}
.newbreak {{ color: {red}; font-size: 11px; font-weight: bold; letter-spacing: 0.5px; }}
.newbreak-rule {{ background-color: alpha({red}, 0.5); min-height: 1px; }}
.chip {{
    background-color: alpha(@sl_fg, 0.07); border: 1px solid alpha(@sl_muted, 0.8);
    border-radius: 10px; padding: 0 7px; font-size: 11px; color: @sl_fg;
}}
.chip.mine {{ background-color: alpha(@sl_accent, 0.22); border-color: @sl_accent; }}
.chip:hover {{ border-color: @sl_accent; }}
.chip.addchip {{ color: @sl_fg_dim; }}
.threadlink {{ color: @sl_accent; font-size: 12px; }}
.threadlink:hover {{ color: @sl_fg_bright; }}
.marks {{ color: @sl_fg_dim; font-size: 11px; }}

/* A text file shown inline. Bordered rather than tinted: a snippet has to be
   distinguishable from the message around it on a light theme and a dark one
   without either being able to rely on a background. */
.snippet {{
    border: 1px solid alpha(@sl_muted, 0.8); border-radius: 6px;
    padding: 6px 8px; margin-top: 4px;
}}
.snippet .code {{
    font-family: monospace; font-size: 12px; color: @sl_fg;
    padding-top: 4px;
}}
.snippet .note {{ color: @sl_fg_dim; font-size: 11px; }}
.snippet expander {{ color: @sl_fg_dim; font-size: 11px; }}

/* The hover bar. It floats over the row's top-right corner, so nothing
   reflows when it appears — a bar that moves the text under the pointer is
   a bar you cannot click. */
.msgactions {{
    background-color: @sl_bg_light; border: 1px solid alpha(@sl_muted, 0.9);
    border-radius: 6px; padding: 1px;
}}
.msgactions button, .moremenu button {{
    background: none; border: none; box-shadow: none;
    min-height: 22px; min-width: 24px; padding: 0 4px;
    color: @sl_fg; font-size: 12px;
}}
.msgactions button:hover, .moremenu button:hover {{
    background-color: alpha(@sl_accent, 0.25);
}}
.moremenu button {{ min-width: 150px; padding: 3px 8px; }}

/* The thread pane: the same conversation, one shade apart from it. */
.threadpane {{ background-color: @sl_bg_light; border-left: 1px solid alpha(@sl_muted, 0.6); }}
.threadpane .header {{ background-color: @sl_bg_light; }}
.threadpane .conversation {{ background-color: @sl_bg_light; }}
.threadpane .composer {{ background-color: @sl_bg; }}
.threadpane checkbutton {{ color: @sl_fg_dim; font-size: 11px; padding: 0 6px 4px 6px; }}

/* The side pane's lists: search results, threads, saved, members. */
.sidelist {{ background-color: @sl_bg_light; }}
.sidelist row {{ border-radius: 5px; margin: 0 4px; }}
.sidelist row:selected {{ background-color: alpha(@sl_accent, 0.22); }}
.sidelist label {{ color: @sl_fg; font-size: 12px; }}
.sidelist .note {{ color: @sl_fg_dim; font-size: 11px; }}

/* Scrollback's own line, above the conversation. */
.loading {{
    color: @sl_fg_dim; font-size: 11px; padding: 4px 12px;
    background-color: @sl_bg;
}}

/* The completion popup: a list under the word being typed. */
.completions {{ background-color: @sl_bg_light; }}
.completions list {{ background-color: @sl_bg_light; }}
.completions row {{ padding: 2px 0; }}
.completions row:selected {{ background-color: alpha(@sl_accent, 0.30); }}
.completions label {{ color: @sl_fg; font-family: {mono}; font-size: 12px; }}

/* The picker and the shortcuts window. */
.picker {{ background-color: @sl_bg; color: @sl_fg; }}
.picker button {{ background: none; border: none; box-shadow: none; font-size: 17px; }}
.picker button:hover {{ background-color: alpha(@sl_accent, 0.25); border-radius: 5px; }}
.picker .section {{ color: @sl_fg_dim; font-size: 11px; font-weight: bold; letter-spacing: 0.5px; }}
.picker .chip {{ font-family: monospace; }}
.blockkit {{
    padding: 8px 10px; border-radius: 6px;
    background-color: alpha(@sl_fg, 0.04);
    border-left: 2px solid alpha(@sl_accent, 0.7);
}}

/* The composer: a panel, not a form field. */
.composer {{
    background-color: @sl_bg_light; border: 1px solid alpha(@sl_muted, 0.9);
    border-radius: 8px; margin: 8px 12px 6px 12px;
}}
.composer:focus-within {{ border-color: @sl_accent; }}
.composer textview, .composer textview text {{
    background: none; color: @sl_fg_bright;
    font-family: {mono}; font-size: {mono_size}px;
}}
.composer .hint {{ color: @sl_fg_dim; font-size: 11px; padding: 0 8px 4px 8px; }}
entry {{
    background-color: @sl_bg_light; color: @sl_fg_bright;
    border: 1px solid alpha(@sl_muted, 0.9); border-radius: 6px;
    caret-color: @sl_accent;
}}
entry:focus {{ border-color: @sl_accent; }}
entry selection, label selection, textview text selection {{
    background-color: @sl_selection; color: @sl_fg_bright;
}}

/* The status bar, in the shape an editor puts it. */
.status {{
    background-color: @sl_bg_darker; color: @sl_fg_dim;
    font-family: monospace; font-size: 11px; padding: 2px 10px;
}}
.status .ok {{ color: {green}; }}
.status .warn {{ color: {yellow}; }}

button {{ background-color: @sl_bg_light; color: @sl_fg; border: 1px solid @sl_muted; border-radius: 6px; }}
button.suggested-action {{ background-color: @sl_accent; color: @sl_bg; border-color: @sl_accent; }}
button.destructive-action {{ background-color: {red}; color: @sl_bg; border-color: {red}; }}
button:disabled {{ color: @sl_fg_dim; }}
button.flat {{ background: none; border: none; box-shadow: none; }}
scrollbar {{ background: none; }}
scrollbar slider {{ background-color: alpha(@sl_muted, 0.8); border-radius: 6px; min-width: 6px; }}
scrollbar slider:hover {{ background-color: @sl_muted; }}
separator {{ background-color: alpha(@sl_muted, 0.5); }}
paned > separator {{ background-color: alpha(@sl_muted, 0.6); min-width: 1px; }}
{authors}"#,
            bg = p.background,
            bgd = p.dark_background,
            bgdd = p.darker_background,
            bgl = p.lighter_background,
            fg = p.foreground,
            fgd = p.dark_foreground,
            fgb = p.bright_foreground,
            acc = p.accent,
            sel = p.selection,
            mut = p.muted,
            red = p.red,
            green = p.green,
            yellow = p.yellow,
            mono = self.mono_family,
            mono_size = self.mono_size,
            authors = authors,
        )
    }
}

/// Apply the palette: one provider, replaced wholesale on every change.
/// `APPLICATION` priority sits above the theme and below `user.css`.
pub struct Applied {
    provider: gtk::CssProvider,
    _user: Option<gtk::CssProvider>,
}

impl Applied {
    /// Apply `pal`, and `user_css` over it at `USER` priority — the one file
    /// a person edits to disagree with the generator, referring to the
    /// `@define-color` names it publishes.
    pub fn new(pal: &Palette, user_css: Option<&Path>) -> Self {
        let display = gtk::gdk::Display::default().expect("display");
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&pal.css());
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let user = user_css.filter(|p| p.is_file()).map(|p| {
            let u = gtk::CssProvider::new();
            u.load_from_path(p);
            gtk::style_context_add_provider_for_display(
                &display,
                &u,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
            u
        });
        set_mode(pal);
        Applied {
            provider,
            _user: user,
        }
    }

    /// A human-readable name for where the palette came from.
    pub fn describe(source: &Source) -> String {
        match source {
            Source::Omarchy(_) => std::fs::read_to_string(omarchy_state_dir().join("theme.name"))
                .map(|s| format!("omarchy/{}", s.trim()))
                .unwrap_or_else(|_| "omarchy".into()),
            Source::File(p) => p.display().to_string(),
            Source::Builtin(n) => format!("built-in {n}"),
        }
    }

    pub fn replace(&self, pal: &Palette) {
        self.provider.load_from_string(&pal.css());
        set_mode(pal);
    }
}

/// The widget theme's light/dark half follows the palette's `mode`.
///
/// `prefer-dark-theme` alone is not enough: Omarchy sets `gtk-theme` to
/// `Adwaita-dark` in gsettings, and a light palette over a dark widget
/// theme leaves every button we do not restyle dark — measured as
/// unreadable grey-on-grey in the light screenshot. So the theme *name* is
/// set for this application, which is what a palette's mode means.
fn set_mode(pal: &Palette) {
    if let Some(settings) = gtk::Settings::default() {
        let light = pal.mode == "light";
        settings.set_gtk_application_prefer_dark_theme(!light);
        settings.set_gtk_theme_name(Some(if light { "Adwaita" } else { "Adwaita-dark" }));
    }
}

/// Watch for a theme change. Monitors the *parent* of the theme directory,
/// because the directory itself is replaced (deleted and recreated) on
/// every `omarchy-theme-set` — a monitor on it would fire once and then
/// watch a path that no longer exists. Debounced, because the swap is
/// several filesystem events in a row and the palette must be read after
/// the last of them.
pub fn watch(on_change: impl Fn() + 'static) -> Option<gtk::gio::FileMonitor> {
    let dir = omarchy_state_dir();
    if !dir.is_dir() {
        return None;
    }
    let file = gtk::gio::File::for_path(&dir);
    let monitor = file
        .monitor_directory(
            gtk::gio::FileMonitorFlags::NONE,
            None::<&gtk::gio::Cancellable>,
        )
        .ok()?;
    let pending = std::rc::Rc::new(std::cell::Cell::new(false));
    let on_change = std::rc::Rc::new(on_change);
    monitor.connect_changed(move |_, changed, _, _| {
        let name = changed.basename().unwrap_or_default();
        if name != Path::new("theme") && name != Path::new("theme.name") {
            return;
        }
        if pending.replace(true) {
            return;
        }
        let pending = pending.clone();
        let on_change = on_change.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
            pending.set(false);
            on_change();
        });
    });
    Some(monitor)
}
