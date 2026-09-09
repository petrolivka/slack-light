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
        format!(
            r#"
@define-color sl_bg {bg};
@define-color sl_bg_dark {bgd};
@define-color sl_bg_light {bgl};
@define-color sl_fg {fg};
@define-color sl_fg_dim {fgd};
@define-color sl_fg_bright {fgb};
@define-color sl_accent {acc};
@define-color sl_selection {sel};
@define-color sl_muted {mut};

window {{ background-color: @sl_bg; color: @sl_fg; }}
.sidebar {{ background-color: @sl_bg_dark; }}
.sidebar row {{ color: @sl_fg; padding: 2px 0; }}
.sidebar row:selected {{ background-color: @sl_selection; color: @sl_fg_bright; }}
.sidebar row:hover {{ background-color: alpha(@sl_selection, 0.6); }}
.badge {{ background-color: {red}; color: @sl_bg; border-radius: 9px; padding: 0 5px; font-weight: bold; }}
.header {{ background-color: @sl_bg; color: @sl_fg_bright; }}
.conversation {{ background-color: @sl_bg; }}
.conversation > row {{ border-bottom: 1px solid alpha(@sl_muted, 0.6); }}
.conversation > row:hover {{ background-color: alpha(@sl_bg_light, 0.5); }}
.conversation label {{ color: @sl_fg; }}
.conversation label:link, .conversation label link {{ color: {blue}; }}
.blockkit {{ padding: 6px 8px; border-left: 3px solid @sl_accent; background-color: alpha(@sl_bg_light, 0.35); }}
entry {{ background-color: @sl_bg_light; color: @sl_fg_bright; border: 1px solid @sl_muted; caret-color: @sl_accent; }}
entry:focus {{ border-color: @sl_accent; }}
entry selection, label selection {{ background-color: @sl_selection; color: @sl_fg_bright; }}
.status {{ color: @sl_fg_dim; background-color: @sl_bg_dark; }}
button {{ background-color: @sl_bg_light; color: @sl_fg; border: 1px solid @sl_muted; }}
button.suggested-action {{ background-color: @sl_accent; color: @sl_bg; border-color: @sl_accent; }}
button.destructive-action {{ background-color: {red}; color: @sl_bg; border-color: {red}; }}
button:disabled {{ color: @sl_fg_dim; }}
scrollbar slider {{ background-color: @sl_muted; }}
separator {{ background-color: @sl_muted; }}
paned > separator {{ background-color: @sl_muted; min-width: 1px; }}
"#,
            bg = p.background,
            bgd = p.dark_background,
            bgl = p.lighter_background,
            fg = p.foreground,
            fgd = p.dark_foreground,
            fgb = p.bright_foreground,
            acc = p.accent,
            sel = p.selection,
            mut = p.muted,
            red = p.red,
            blue = p.blue,
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
