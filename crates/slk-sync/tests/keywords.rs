//! Highlight words: what counts as one, and what does not.
//!
//! A keyword that fires on a substring fires constantly, and a user whose
//! notifications are all false gets rid of them along with the real ones. So
//! the whole-word rule is the feature, not a detail of it.

use slk_sync::keyword_matches;

#[test]
fn a_whole_word_matches() {
    let k = ["deploy".to_string()];
    assert!(keyword_matches(&k, "the deploy is done"));
    assert!(keyword_matches(&k, "deploy"));
    assert!(keyword_matches(&k, "Deploy at nine"), "case is ignored");
    assert!(
        keyword_matches(&k, "ready to deploy."),
        "punctuation ends it"
    );
    assert!(keyword_matches(&k, "(deploy)"));
}

#[test]
fn a_substring_does_not() {
    let k = ["ops".to_string()];
    assert!(!keyword_matches(&k, "a developer wrote it"));
    assert!(!keyword_matches(&k, "oops"));
    assert!(!keyword_matches(&k, "opsgenie"));
    assert!(keyword_matches(&k, "ask ops about it"));
}

#[test]
fn an_underscore_is_part_of_a_word() {
    // `deploy_bot` is a name, not a mention of `deploy`.
    let k = ["deploy".to_string()];
    assert!(!keyword_matches(&k, "deploy_bot said so"));
    assert!(!keyword_matches(&k, "re_deploy"));
}

#[test]
fn any_of_several_will_do() {
    let k = ["ops".to_string(), "incident".to_string()];
    assert!(keyword_matches(&k, "an incident, maybe"));
    assert!(keyword_matches(&k, "ops please"));
    assert!(!keyword_matches(&k, "nothing to see"));
}

#[test]
fn no_keywords_match_nothing() {
    assert!(!keyword_matches(&[], "deploy ops incident"));
}

#[test]
fn a_multi_word_keyword_works_too() {
    let k = ["on call".to_string()];
    assert!(keyword_matches(&k, "who is on call tonight"));
    assert!(!keyword_matches(&k, "on calls tonight"));
}
