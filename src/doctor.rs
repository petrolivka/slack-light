//! `--doctor`: what this machine and this desktop can actually do.
//!
//! The rule from the very first spike applies: a capability check must
//! distinguish "answered no" from "did not answer", and say which. Every
//! probe here is bounded, and none opens a window.

use anyhow::Result;

pub fn report(anonymous: bool) -> Result<()> {
    println!("slack-light {}\n", env!("CARGO_PKG_VERSION"));

    println!("desktop");
    println!(
        "  gtk             {}.{}.{}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version()
    );
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "(unset)".into());
    let wayland = std::env::var("WAYLAND_DISPLAY").ok();
    println!(
        "  display         {}{}",
        session,
        wayland
            .as_deref()
            .map(|w| format!(" ({w})"))
            .unwrap_or_default()
    );
    let compositor = if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        "Hyprland".to_string()
    } else {
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "(unknown)".into())
    };
    println!("  compositor      {compositor}");
    println!(
        "  portal          {}",
        dbus_name_owned("org.freedesktop.portal.Desktop")
    );
    println!(
        "  notifications   {}",
        dbus_name_owned("org.freedesktop.Notifications")
    );
    println!("  a11y bus        {}", dbus_name_owned("org.a11y.Bus"));
    // The three-way answer FR-K7 asks for: watching, told no, or could not
    // ask. This binds and immediately drops the connection, so it costs a
    // round trip and leaves nothing behind.
    {
        let (tx, _rx) = std::sync::mpsc::channel();
        let support = slk_idle::watch(std::time::Duration::from_secs(600), tx);
        println!("  idle → away     {}", support.describe());
    }

    println!("\ntheme");
    let (config, _) = slk_config::Config::load();
    let omarchy = slk_theme::omarchy_state_dir().join("theme/colors.toml");
    println!(
        "  omarchy         {}",
        if omarchy.is_file() {
            format!(
                "yes — {}",
                std::fs::read_to_string(slk_theme::omarchy_state_dir().join("theme.name"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "(unnamed)".into())
            )
        } else {
            "no".into()
        }
    );
    println!(
        "  configured      source={} builtin={}{}",
        config.theme.source,
        config.theme.builtin,
        if config.theme.file.is_empty() {
            String::new()
        } else {
            format!(" file={}", config.theme.file)
        }
    );
    let user_css = slk_config::config_dir().join("user.css");
    println!(
        "  user.css        {}",
        if user_css.is_file() {
            "present"
        } else {
            "none"
        }
    );

    println!("\nsign-in");
    match slk_auth::browser::find_browser(None) {
        Ok(p) => println!("  browser         {}", p.display()),
        Err(_) => println!("  browser         none found (chromium, google-chrome, brave) — `auth add --paste` still works"),
    }

    println!("\nconfiguration");
    println!("  config          {}", slk_config::Config::path().display());
    println!(
        "  cache           {}",
        slk_config::data_dir().join("store.sqlite").display()
    );
    println!(
        "  logs            {}",
        slk_config::state_dir().join("slack-light.log").display()
    );
    println!("  keymap          preset={}", config.keymap.preset);

    println!("\naccount");
    if anonymous {
        println!("  --anonymous, so no credentials were read");
    } else {
        match slk_auth::load_all() {
            Ok(all) => {
                println!("  credentials     {} (0600)", slk_auth::path().display());
                for a in all {
                    println!("  workspace       {}", a.team);
                }
            }
            Err(_) => println!("  none stored. Run `slack-light auth add`."),
        }
    }

    println!("\ncache");
    match slk_store::Store::open(Some(&slk_config::data_dir().join("store.sqlite"))) {
        Ok(s) => match s.stats() {
            Ok(st) => println!("  messages held   {}", st.messages),
            Err(e) => println!("  unreadable: {e}"),
        },
        Err(e) => println!("  none ({e})"),
    }
    Ok(())
}

/// Whether a well-known name has an owner on the session bus — bounded,
/// and "no bus" is reported as such rather than as "no".
fn dbus_name_owned(name: &str) -> &'static str {
    let Ok(bus) =
        gtk::gio::bus_get_sync(gtk::gio::BusType::Session, None::<&gtk::gio::Cancellable>)
    else {
        return "no session bus";
    };
    let reply = bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&(name,).to_variant()),
        None,
        gtk::gio::DBusCallFlags::NONE,
        1000,
        None::<&gtk::gio::Cancellable>,
    );
    use gtk::glib::prelude::*;
    match reply {
        Ok(v) => {
            if v.child_value(0).get::<bool>() == Some(true) {
                "yes"
            } else {
                "no"
            }
        }
        Err(_) => "did not answer",
    }
}
