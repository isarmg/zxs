use super::*;
use crate::fs::Entity;

#[test]
fn log_entry_is_private_regular_and_on_the_held_parent_filesystem() {
    let parent = Place { entity: Entity { device: 3, inode: 5 }, mount: 9 };
    let file = Place { entity: Entity { device: 3, inode: 6 }, ..parent };
    let s = Stamp { place: file, bytes: 0, ctime: (0, 0), mtime: (0, 0),
                    mode: 0o100600, uid: 0, gid: 0 };
    assert!(eligible(s, parent));
    for other in [Stamp { uid: 1, ..s }, Stamp { gid: 1, ..s }, Stamp { mode: 0o100644, ..s },
                  Stamp { mode: 0o120600, ..s },
                  Stamp { place: Place { mount: 10, ..file }, ..s },
                  Stamp { place: Place { entity: Entity { device: 4, ..file.entity }, ..file }, ..s }] {
        assert!(!eligible(other, parent));
    }
}

#[test]
fn emitted_fields_are_only_a_timestamp_and_a_closed_lifecycle_value() {
    for (event, expected) in [(Event::ForegroundSelected, "original-selected-not-started"),
                              (Event::PairDrained, "web-smb-drained"),
                              (Event::RunUnconfirmed, "start-or-drain-unconfirmed")] {
        assert_eq!(event.line(Some(7)), format!("zxs time_unix=7 lifecycle={expected}\n"));
        assert_eq!(event.line(None), format!("zxs time_unix=unknown lifecycle={expected}\n"));
    }
}
