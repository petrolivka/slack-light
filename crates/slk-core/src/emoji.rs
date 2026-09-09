//! Slack shortcodes to Unicode.
//!
//! Slack's shortcode set is `emoji-data`'s (iamcal), which overlaps gemoji's
//! heavily but not completely — and M0 measured exactly where they diverge:
//! Slack says `man-woman-girl-boy`, `flag-cz`, `rainbow-flag` and
//! `thinking_face` where gemoji says `family_man_woman_girl_boy`,
//! `czech_republic`, `rainbow_flag` and `thinking`.
//!
//! The `emojis` crate indexes gemoji, so it resolves most names and misses
//! those. A vendored `emoji-data` table generated in `build.rs` is the real
//! fix and is M2 work; until then the aliases below cover the measured gaps and
//! anything still unresolved renders as `:name:`, which is also what a
//! workspace's custom emoji does.

/// Names Slack uses that the gemoji-backed lookup does not know, mapped to a
/// name it does. Every entry here was observed in a real response.
const SLACK_ALIASES: &[(&str, &str)] = &[
    ("man-woman-girl-boy", "family_man_woman_girl_boy"),
    ("man-woman-girl", "family_man_woman_girl"),
    ("man-woman-boy-boy", "family_man_woman_boy_boy"),
    ("woman-woman-girl", "family_woman_woman_girl"),
    ("man-man-boy", "family_man_man_boy"),
    ("rainbow-flag", "rainbow_flag"),
    ("thinking_face", "thinking"),
    ("simple_smile", "slightly_smiling_face"),
    ("flag-cz", "czech_republic"),
    ("flag-us", "us"),
    ("flag-gb", "gb"),
    ("flag-de", "de"),
    ("flag-fr", "fr"),
    ("flag-jp", "jp"),
    ("flag-sk", "slovakia"),
    ("flag-pl", "poland"),
    ("flag-ua", "ukraine"),
];

/// Resolve a shortcode, applying a skin tone if one was given.
///
/// Returns `None` for a name no table knows — a custom workspace emoji, or one
/// Slack added since this table was built. Callers render `:name:` in that case
/// rather than dropping it.
pub fn shortcode(name: &str, skin: Option<u8>) -> Option<String> {
    let looked_up = emojis::get_by_shortcode(name).or_else(|| {
        SLACK_ALIASES
            .iter()
            .find(|(slack, _)| *slack == name)
            .and_then(|(_, gemoji)| emojis::get_by_shortcode(gemoji))
    })?;

    let Some(n) = skin else {
        return Some(looked_up.as_str().to_string());
    };
    // Slack numbers tones 2..6 after the `:skin-tone-N:` modifier codepoints.
    let tone = match n {
        2 => emojis::SkinTone::Light,
        3 => emojis::SkinTone::MediumLight,
        4 => emojis::SkinTone::Medium,
        5 => emojis::SkinTone::MediumDark,
        _ => emojis::SkinTone::Dark,
    };
    Some(
        looked_up
            .with_skin_tone(tone)
            .unwrap_or(looked_up)
            .as_str()
            .to_string(),
    )
}

/// The reactions worth a single keystroke. Slack's own most-used set, and the
/// order people expect to find them in.
pub const QUICK: &[&str] = &["+1", "tada", "white_check_mark", "eyes", "heart"];

/// What the picker offers before anything is typed.
///
/// Iterating the Unicode table and sorting by name length produces a screen of
/// near-identical smileys, which is a list nobody wanted. These are the ones
/// people actually react with.
const COMMON: &[&str] = &[
    "+1",
    "-1",
    "tada",
    "white_check_mark",
    "eyes",
    "heart",
    "fire",
    "rocket",
    "clap",
    "raised_hands",
    "pray",
    "thinking",
    "sob",
    "joy",
    "100",
    "wave",
    "point_up",
    "warning",
    "x",
    "question",
    "bulb",
    "sparkles",
    "muscle",
    "beers",
    "coffee",
    "brain",
    "chart_with_upwards_trend",
    "bug",
    "wrench",
    "lock",
];

/// Shortcodes matching a query, best first.
///
/// A prefix match beats a substring one, because typing `ta` should reach
/// `tada` before `guitar`. Custom workspace emoji are layered on by the caller,
/// which is the only part of the set this crate cannot know.
pub fn search(query: &str, limit: usize) -> Vec<(String, String)> {
    search_with(query, limit, None)
}

/// The same, showing every result in one skin tone.
///
/// Only the glyph changes; the name stays as Slack knows it, and the tone is
/// added when the reaction is sent. Otherwise the picker would show one thing
/// and react with another.
pub fn search_with(query: &str, limit: usize, skin: Option<u8>) -> Vec<(String, String)> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return COMMON
            .iter()
            .filter_map(|n| shortcode(n, skin).map(|g| (n.to_string(), g)))
            .take(limit)
            .collect();
    }

    let mut prefix = Vec::new();
    let mut substring = Vec::new();

    for e in emojis::iter() {
        for code in e.shortcodes() {
            if code == q {
                prefix.push((code.to_string(), e.as_str().to_string()));
                break;
            }
            if code.starts_with(&q) {
                prefix.push((code.to_string(), e.as_str().to_string()));
                break;
            }
            if code.contains(&q) {
                substring.push((code.to_string(), e.as_str().to_string()));
                break;
            }
        }
    }
    // Shortest first, but the curated set first of all: typing `roc` means
    // :rocket: far more often than :rock:, and shortest-prefix alone put the
    // rock ahead of it.
    let common_first =
        |(code, _): &(String, String)| (!COMMON.contains(&code.as_str()), code.len());
    prefix.sort_by_key(common_first);
    substring.sort_by_key(common_first);
    prefix.extend(substring);
    prefix.truncate(limit);
    // Only the ones that have a toned form change; a rocket stays a rocket.
    if skin.is_some() {
        for (name, glyph) in prefix.iter_mut() {
            if let Some(toned) = shortcode(name, skin) {
                *glyph = toned;
            }
        }
    }
    prefix
}

/// The name to send for a reaction, with the tone Slack expects on it.
///
/// Slack writes a toned reaction as `wave::skin-tone-3`, and only for emoji
/// that actually have tones — sending it on a rocket is refused.
pub fn with_tone(name: &str, skin: Option<u8>) -> String {
    match skin.filter(|n| (2..=6).contains(n)) {
        Some(n) if has_tones(name) => format!("{name}::skin-tone-{n}"),
        _ => name.to_string(),
    }
}

/// Whether this emoji has skin-toned forms at all.
fn has_tones(name: &str) -> bool {
    emojis::get_by_shortcode(name)
        .map(|e| e.skin_tones().is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tone_tests {
    #[test]
    fn the_common_set_wins_a_tie_on_prefix() {
        let hits = super::search("roc", 8);
        let names: Vec<&str> = hits.iter().map(|(n, _)| n.as_str()).collect();
        let rocket = names.iter().position(|n| *n == "rocket").unwrap();
        let rock = names.iter().position(|n| *n == "rock").unwrap();
        assert!(rocket < rock, "rocket before rock: {names:?}");
    }

    #[test]
    fn a_tone_is_added_only_where_there_is_one() {
        assert_eq!(super::with_tone("wave", Some(3)), "wave::skin-tone-3");
        assert_eq!(super::with_tone("rocket", Some(3)), "rocket");
        assert_eq!(super::with_tone("wave", None), "wave");
        // Slack numbers them 2 to 6; anything else is the yellow default.
        assert_eq!(super::with_tone("wave", Some(0)), "wave");
        assert_eq!(super::with_tone("wave", Some(9)), "wave");
    }

    #[test]
    fn the_picker_shows_the_tone_it_will_send() {
        let plain = super::search_with("wave", 4, None);
        let toned = super::search_with("wave", 4, Some(5));
        let a = plain.iter().find(|(n, _)| n == "wave").unwrap();
        let b = toned.iter().find(|(n, _)| n == "wave").unwrap();
        assert_ne!(a.1, b.1, "the glyph should carry the tone");
        assert_eq!(b.0, "wave", "the name should not");
    }
}
