//! Telling the user something happened while they were not looking.
//!
//! Three channels, independently switchable, because they fail in different
//! places. A desktop notification needs a session bus and is useless over SSH.
//! The terminal bell reaches a multiplexer on the other end of a link but is
//! easy to miss. The OSC sequences ask the terminal emulator itself to raise a
//! notification, which is the one that works over SSH into a modern terminal.
//!
//! What is *worth* notifying about is not decided here. That belongs with the
//! code that knows whether the conversation is muted and whether the user is
//! already looking at it.

use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc {
    /// Off.
    None,
    /// OSC 9: one string, understood by kitty, wezterm and Windows Terminal.
    Nine,
    /// OSC 777: title and body, understood by foot, urxvt and others.
    SevenSevenSeven,
    /// Pick from `$TERM` and `$TERM_PROGRAM`.
    Auto,
}

impl Osc {
    pub fn parse(s: &str) -> Self {
        match s {
            "osc9" => Osc::Nine,
            "osc777" => Osc::SevenSevenSeven,
            "off" => Osc::None,
            _ => Osc::Auto,
        }
    }

    fn resolve(self) -> Osc {
        if self != Osc::Auto {
            return self;
        }
        let term = std::env::var("TERM").unwrap_or_default();
        let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
        // foot documents 777; kitty and wezterm document 9. Anything else gets
        // nothing rather than a stray escape sequence painted into the pane.
        if term.contains("foot") {
            Osc::SevenSevenSeven
        } else if term.contains("kitty")
            || program.contains("WezTerm")
            || program.contains("ghostty")
        {
            Osc::Nine
        } else {
            Osc::None
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub desktop: bool,
    pub bell: bool,
    pub osc: Osc,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            desktop: true,
            bell: false,
            osc: Osc::Auto,
        }
    }
}

pub struct Notifier {
    config: Config,
    osc: Osc,
    /// Whether the desktop channel has already failed. One failure means no
    /// session bus, which will not fix itself, and retrying every message
    /// would cost a D-Bus timeout each time.
    desktop_broken: bool,
}

impl Notifier {
    pub fn new(config: Config) -> Self {
        let osc = config.osc.resolve();
        Notifier {
            config,
            osc,
            desktop_broken: false,
        }
    }

    /// Raise a notification through every channel that is switched on.
    ///
    /// Never fails: a notification that cannot be delivered must not interrupt
    /// what the user was doing.
    pub fn notify(&mut self, title: &str, body: &str) {
        if self.config.desktop && !self.desktop_broken {
            let r = notify_rust::Notification::new()
                .summary(title)
                .body(body)
                .appname("slack-light")
                .timeout(notify_rust::Timeout::Milliseconds(6000))
                .show();
            if let Err(e) = r {
                tracing::debug!("desktop notification unavailable: {e}");
                self.desktop_broken = true;
            }
        }

        // These go to the terminal, which is also where the interface is
        // drawn. They are written directly rather than through ratatui, which
        // has no idea what they are, and flushed so a bell is not sitting in a
        // buffer until the next frame.
        let mut out = String::new();
        if self.config.bell {
            out.push('\x07');
        }
        match self.osc {
            Osc::Nine => out.push_str(&format!("\x1b]9;{title}: {body}\x07")),
            Osc::SevenSevenSeven => out.push_str(&format!("\x1b]777;notify;{title};{body}\x07")),
            _ => {}
        }
        if !out.is_empty() {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(out.as_bytes());
            let _ = stdout.flush();
        }
    }

    /// What `--doctor` should say about where a notification would go.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.config.desktop {
            parts.push("desktop".to_string());
        }
        if self.config.bell {
            parts.push("bell".to_string());
        }
        match self.osc {
            Osc::Nine => parts.push("OSC 9".into()),
            Osc::SevenSevenSeven => parts.push("OSC 777".into()),
            _ => {}
        }
        if parts.is_empty() {
            "nothing enabled".into()
        } else {
            parts.join(", ")
        }
    }
}
