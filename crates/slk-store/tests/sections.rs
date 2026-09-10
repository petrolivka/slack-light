//! The person's own sidebar sections, and the bookmark bar: two lists the
//! store holds whole and replaces whole.

use slk_core::{Bookmark, ChannelId, SectionKind, SidebarSection, TeamId};
use slk_store::Store;

fn section(id: &str, name: &str, kind: SectionKind, chans: &[&str]) -> SidebarSection {
    SidebarSection {
        id: id.into(),
        name: name.into(),
        emoji: String::new(),
        kind,
        channels: chans.iter().map(|c| ChannelId::new(*c)).collect(),
    }
}

#[test]
fn sections_keep_their_order_and_a_second_write_replaces_the_first() {
    let store = Store::open(None).expect("an in-memory store");
    let team = TeamId::new("T0REAL");
    let first = vec![
        section("L2", "Projects", SectionKind::Custom, &["C1", "C2"]),
        section("L1", "", SectionKind::Channels, &[]),
        section("L9", "", SectionKind::Other, &[]),
    ];
    store.set_sections(&team, &first).unwrap();
    assert_eq!(
        store.sections(&team).unwrap(),
        first,
        "order, kinds and members, as written"
    );

    // Somebody deleted "Projects" on their phone. It has to leave here too.
    let second = vec![section("L1", "", SectionKind::Channels, &[])];
    store.set_sections(&team, &second).unwrap();
    assert_eq!(store.sections(&team).unwrap(), second);
}

#[test]
fn the_demo_leaves_no_sections_or_bookmarks_behind() {
    let store = Store::open(None).expect("an in-memory store");
    let demo = TeamId::new("T0MOCK");
    let real = TeamId::new("T0REAL");
    let s = vec![section("L2", "Projects", SectionKind::Custom, &["C1"])];
    let b = vec![Bookmark {
        id: "Bk1".into(),
        title: "Runbook".into(),
        link: "https://example.invalid/r".into(),
        emoji: String::new(),
    }];
    for t in [&demo, &real] {
        store.set_sections(t, &s).unwrap();
        store.set_bookmarks(t, &ChannelId::new("C1"), &b).unwrap();
    }
    store.forget_demo().unwrap();
    assert!(store.sections(&demo).unwrap().is_empty());
    assert!(store
        .bookmarks(&demo, &ChannelId::new("C1"))
        .unwrap()
        .is_empty());
    // And only the demo's: a sweep that took the real workspace's with it
    // would be a worse bug than the leak it cleans up.
    assert_eq!(store.sections(&real).unwrap(), s);
    assert_eq!(store.bookmarks(&real, &ChannelId::new("C1")).unwrap(), b);
}
