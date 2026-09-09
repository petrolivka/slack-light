//! The decisions the window makes, with no widget in sight.
//!
//! Spike D3: relm4's `ComponentSender` cannot be constructed outside a
//! running component, so `update()` cannot be driven from a test. The
//! answer is not a test harness for relm4; it is to keep the decisions out
//! of `update()` in the first place. These are the ones the shell makes,
//! and `update()` calls them.

/// Which conversation to open first: one with a mention, else one with
/// anything unread, else the first. Muted ones never win a badge.
pub fn landing<'a>(convs: impl IntoIterator<Item = (&'a bool, &'a u32, &'a u32)>) -> usize {
    let convs: Vec<(bool, u32, u32)> = convs.into_iter().map(|(m, u, n)| (*m, *u, *n)).collect();
    convs
        .iter()
        .position(|(muted, _, mentions)| *mentions > 0 && !muted)
        .or_else(|| {
            convs
                .iter()
                .position(|(muted, unread, _)| *unread > 0 && !muted)
        })
        .unwrap_or(0)
}

/// Where an upserted message goes in the list: over the row it replaces
/// (the optimistic one, or an edit of itself), or at the end.
#[derive(Debug, PartialEq, Eq)]
pub enum Placement {
    Replace(usize),
    Append,
}

pub fn placement(rows: &[&str], ts: &str, replaces: Option<&str>) -> Placement {
    let target = replaces.unwrap_or(ts);
    match rows.iter().position(|r| *r == target) {
        Some(i) => Placement::Replace(i),
        None => Placement::Append,
    }
}

/// The next row to select from the keyboard, clamped: alt-Down on the last
/// conversation stays there rather than wrapping, because wrapping is the
/// thing that sends a reply to the wrong channel.
pub fn step(selected: Option<usize>, len: usize, down: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (selected, down) {
        (None, _) => 0,
        (Some(i), true) => (i + 1).min(len - 1),
        (Some(i), false) => i.saturating_sub(1),
    })
}

/// A rendered `slk-config` chord ("ctrl+k", "alt+up", "f1", "esc") as a
/// GTK accelerator ("<Control>k", "<Alt>Up", "F1", "Escape").
pub fn accel(rendered: &str) -> Option<String> {
    let mut mods = String::new();
    let mut key = None;
    for part in rendered.split('+') {
        match part {
            "ctrl" => mods.push_str("<Control>"),
            "alt" => mods.push_str("<Alt>"),
            "shift" => mods.push_str("<Shift>"),
            "" => key = Some("plus".to_string()),
            k => {
                key = Some(match k {
                    "enter" => "Return".into(),
                    "esc" => "Escape".into(),
                    "tab" => "Tab".into(),
                    "space" => "space".into(),
                    "backspace" => "BackSpace".into(),
                    "up" | "down" | "left" | "right" | "home" | "end" | "delete" => {
                        let mut c = k.chars();
                        c.next().unwrap().to_uppercase().collect::<String>() + c.as_str()
                    }
                    "pageup" => "Page_Up".into(),
                    "pagedown" => "Page_Down".into(),
                    f if f.starts_with('f') && f[1..].parse::<u8>().is_ok() => f.to_uppercase(),
                    c if c.chars().count() == 1 => c.to_string(),
                    _ => return None,
                })
            }
        }
    }
    key.map(|k| format!("{mods}{k}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_prefers_a_mention_then_unread_then_first() {
        let c = [(false, 0u32, 0u32), (false, 3, 0), (false, 1, 2)];
        assert_eq!(landing(c.iter().map(|(m, u, n)| (m, u, n))), 2);
        let c = [(false, 0u32, 0u32), (false, 3, 0), (false, 1, 0)];
        assert_eq!(landing(c.iter().map(|(m, u, n)| (m, u, n))), 1);
        let c = [(false, 0u32, 0u32), (false, 0, 0)];
        assert_eq!(landing(c.iter().map(|(m, u, n)| (m, u, n))), 0);
    }

    #[test]
    fn a_muted_conversation_never_wins() {
        let c = [(true, 5u32, 5u32), (false, 1, 0)];
        assert_eq!(landing(c.iter().map(|(m, u, n)| (m, u, n))), 1);
    }

    #[test]
    fn an_optimistic_row_is_replaced_in_place() {
        let rows = ["1.0", "2.0", "local-3"];
        assert_eq!(
            placement(&rows, "3.0", Some("local-3")),
            Placement::Replace(2)
        );
        assert_eq!(
            placement(&rows, "2.0", None),
            Placement::Replace(1),
            "an edit"
        );
        assert_eq!(placement(&rows, "4.0", None), Placement::Append);
        assert_eq!(placement(&rows, "4.0", Some("gone")), Placement::Append);
    }

    #[test]
    fn stepping_clamps_at_both_ends() {
        assert_eq!(step(None, 3, true), Some(0));
        assert_eq!(step(Some(2), 3, true), Some(2));
        assert_eq!(step(Some(0), 3, false), Some(0));
        assert_eq!(step(Some(1), 3, false), Some(0));
        assert_eq!(step(Some(0), 0, true), None);
    }

    #[test]
    fn chords_become_accelerators() {
        assert_eq!(accel("ctrl+k").as_deref(), Some("<Control>k"));
        assert_eq!(accel("alt+up").as_deref(), Some("<Alt>Up"));
        assert_eq!(accel("f1").as_deref(), Some("F1"));
        assert_eq!(accel("esc").as_deref(), Some("Escape"));
        assert_eq!(accel("ctrl+shift+p").as_deref(), Some("<Control><Shift>p"));
        assert_eq!(accel("enter").as_deref(), Some("Return"));
        assert_eq!(accel("ctrl+1").as_deref(), Some("<Control>1"));
        assert_eq!(accel("nonsense"), None);
    }

    #[test]
    fn every_built_in_palette_parses_and_colours_the_css() {
        for text in [
            include_str!("../themes/dark.toml"),
            include_str!("../themes/light.toml"),
        ] {
            let p: crate::theme::Palette = toml::from_str(text).unwrap();
            let css = p.css();
            assert!(
                css.contains(&p.background),
                "background colour reaches the css"
            );
            assert!(css.contains(&p.accent));
            assert!(css.contains("@define-color sl_bg"));
        }
    }
}
