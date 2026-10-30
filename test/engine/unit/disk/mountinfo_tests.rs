use super::*;

fn record(point: &str, source: &str) -> String {
    format!("36 35 8:1 / {point} rw,relatime - btrfs {source} rw\n")
}

#[test]
fn mount_point_decodes_tab_before_matching() {
    let mount = resolve(
        &record(r"/mnt/tab\011disk", "/dev/sda1"),
        Path::new("/mnt/tab\tdisk/data"),
    )
    .unwrap();
    assert_eq!(mount.point, Path::new("/mnt/tab\tdisk"));
}

#[test]
fn mount_point_decodes_newline_before_matching() {
    let mount = resolve(
        &record(r"/mnt/line\012disk", "/dev/sda1"),
        Path::new("/mnt/line\ndisk/data"),
    )
    .unwrap();
    assert_eq!(mount.point, Path::new("/mnt/line\ndisk"));
}

#[test]
fn mount_source_decodes_kernel_path_escapes() {
    let mount = resolve(
        &record("/mnt/disk", r"/dev/disk\040space\011tab\012line\134slash"),
        Path::new("/mnt/disk/data"),
    )
    .unwrap();
    assert_eq!(mount.source, Path::new("/dev/disk space\ttab\nline\\slash"));
}

#[test]
fn all_path_escapes_are_decoded_once_even_when_adjacent() {
    let encoded = r"/mnt/\040\011\012\134\134040\134011\134012\134134";
    let decoded = "/mnt/ \t\n\\\\040\\011\\012\\134";
    let mount = resolve(&record(encoded, encoded), Path::new(decoded)).unwrap();
    assert_eq!(mount.point, Path::new(decoded));
    assert_eq!(mount.source, Path::new(decoded));
}

#[test]
fn kernel_path_escape_roundtrips_preserve_literal_escape_prefixes() {
    fn encode(path: &str) -> String {
        path.chars()
            .map(|ch| match ch {
                ' ' => r"\040".to_owned(),
                '\t' => r"\011".to_owned(),
                '\n' => r"\012".to_owned(),
                '\\' => r"\134".to_owned(),
                _ => ch.to_string(),
            })
            .collect()
    }
    let pieces = [" ", "\t", "\n", "\\", "040", "134", "\r", "盘\u{a0}#"];
    for a in pieces {
        for b in pieces {
            for c in pieces {
                let path = format!("/mnt/{a}{b}{c}");
                let encoded = encode(&path);
                let mount = resolve(&record(&encoded, &encoded), Path::new(&path)).unwrap();
                assert_eq!(mount.point, Path::new(&path));
                assert_eq!(mount.source, Path::new(&path));
            }
        }
    }
}

#[test]
fn selects_longest_decoded_mount_point_independent_of_record_order() {
    let rows = [
        record("/", "/dev/root"),
        record(r"/mnt/disk\040one", "/dev/parent"),
        record(r"/mnt/disk\040one/nested\011mount", "/dev/child"),
        record(r"/mnt/disk\040one/nested\011mount/sibling", "/dev/sibling"),
    ];
    let path = Path::new("/mnt/disk one/nested\tmount/data");
    for table in [rows.concat(), rows.into_iter().rev().collect::<String>()] {
        let mount = resolve(&table, path).unwrap();
        assert_eq!(mount.point, Path::new("/mnt/disk one/nested\tmount"));
        assert_eq!(mount.source, Path::new("/dev/child"));
        assert_eq!(mount.kind, "btrfs");
    }
}

#[test]
fn matching_respects_path_components_not_text_prefixes() {
    let table = record("/mnt", "/dev/parent") + &record("/mnt/disk", "/dev/disk");
    for path in ["/mnt/disk2", "/mnt/disk-sibling/data"] {
        let mount = resolve(&table, Path::new(path)).unwrap();
        assert_eq!(mount.point, Path::new("/mnt"));
    }
    let mount = resolve(&table, Path::new("/mnt/disk")).unwrap();
    assert_eq!(mount.source, Path::new("/dev/disk"));
}

#[test]
fn ignores_unknown_optional_fields_and_decodes_subvolume_root() {
    let table = concat!(
        "36 35 0:42 /sub\\040vol\\011tab\\012line\\134slash /mnt/disk rw ",
        "shared:7 future_tag:value unbindable - btrfs /dev/sda1 rw,subvolid=256\n",
    );
    let mount = resolve(table, Path::new("/mnt/disk/data")).unwrap();
    assert_eq!(mount.source, Path::new("/dev/sda1"));
}

#[test]
fn filesystem_specific_root_labels_do_not_hide_other_mounts() {
    // fs/nsfs.c::nsfs_show_path emits a namespace label, not an absolute path.
    let table = "85 27 0:4 net:[4026531840] /run/netns/test rw - nsfs nsfs rw\n".to_owned()
        + &record("/mnt/disk", "/dev/sda1");
    let mount = resolve(&table, Path::new("/mnt/disk/data")).unwrap();
    assert_eq!(mount.source, Path::new("/dev/sda1"));
    assert_eq!(mount.kind, "btrfs");
}

#[test]
fn preserves_carriage_return_and_unicode_whitespace_in_paths() {
    let path = "/mnt/盘\r\u{a0}\u{2003}";
    let mount = resolve(&record(path, path), Path::new(path)).unwrap();
    assert_eq!(mount.point, Path::new(path));
    assert_eq!(mount.source, Path::new(path));
}

#[test]
fn does_not_select_an_ancestor_to_hide_an_unsupported_filesystem() {
    let table = record("/", "/dev/sda1") + "37 36 0:43 / /mnt/disk rw - tmpfs none rw\n";
    let mount = resolve(&table, Path::new("/mnt/disk/data")).unwrap();
    assert_eq!(mount.kind, "tmpfs");
    assert_eq!(mount.source, Path::new("none"));
}

#[test]
fn accepts_the_generic_mount_source_hash_escape_without_decoding_it_twice() {
    let table = "36 35 8:1 / /mnt/disk rw - ext4 /dev/disk\\043name\\134043 rw\n";
    let mount = resolve(table, Path::new("/mnt/disk")).unwrap();
    assert_eq!(mount.source, Path::new("/dev/disk#name\\043"));
}

#[test]
fn rejects_malformed_or_unsupported_path_escapes() {
    for suffix in [
        r"\", r"\0", r"\04", r"\000", r"\057", r"\141", r"\999", r"\xyz", "\0",
    ] {
        let path = format!("/mnt/bad{suffix}");
        for table in [
            record(&path, "/dev/sda1"),
            record("/mnt/disk", &path),
            format!("36 35 8:1 {path} /mnt/disk rw - btrfs /dev/sda1 rw\n"),
        ] {
            let error = resolve(&table, Path::new("/mnt/disk")).unwrap_err();
            assert_eq!(
                error.code(),
                storage_contract::ErrorCode::DiskIdentityMismatch,
                "{table:?}"
            );
        }
    }
    // Unlike a generic source, a kernel mount-point path does not escape '#'.
    assert!(resolve(&record(r"/mnt/\043", "/dev/sda1"), Path::new("/mnt/#")).is_err());
}

#[test]
fn malformed_rows_cannot_fall_back_to_an_ancestor_mount() {
    let malformed = [
        "",
        "36 35 8:1 / /mnt/disk rw btrfs /dev/sda1 rw",
        "36 35 8:1 / /mnt/disk - btrfs /dev/sda1 rw",
        "36 35 8:1 / /mnt/disk rw - btrfs /dev/sda1",
        "36 35 8:1 / /mnt/disk rw - btrfs /dev/sda1 rw extra",
        "36  35 8:1 / /mnt/disk rw - btrfs /dev/sda1 rw",
        "id 35 8:1 / /mnt/disk rw - btrfs /dev/sda1 rw",
        "36 parent 8:1 / /mnt/disk rw - btrfs /dev/sda1 rw",
        "36 35 8:x / /mnt/disk rw - btrfs /dev/sda1 rw",
        "36 35 8:1:2 / /mnt/disk rw - btrfs /dev/sda1 rw",
        "36 35 8:1 / relative rw - btrfs /dev/sda1 rw",
        "36 35 8:1 / /mnt/disk\tname rw - btrfs /dev/sda1 rw",
        "36 35 8:1 / /mnt/disk\nname rw - btrfs /dev/sda1 rw",
    ];
    for row in malformed {
        let table = format!("{}{row}\n", record("/", "/dev/root"));
        let error = resolve(&table, Path::new("/mnt/disk/data")).unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::DiskIdentityMismatch,
            "{row:?}"
        );
    }
}

#[test]
fn rejects_empty_tables_unmatched_roots_and_relative_lookup_paths() {
    for (table, path) in [
        (String::new(), "/mnt/disk"),
        (record("/other", "/dev/sda1"), "/mnt/disk"),
        (record("/", "/dev/sda1"), "relative"),
    ] {
        let error = resolve(&table, Path::new(path)).unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::DiskIdentityMismatch
        );
    }
}

#[test]
fn accepts_a_complete_record_without_a_trailing_newline() {
    let table = record("/mnt/disk", "/dev/sda1");
    let mount = resolve(table.strip_suffix('\n').unwrap(), Path::new("/mnt/disk")).unwrap();
    assert_eq!(mount.source, Path::new("/dev/sda1"));
}

#[test]
fn held_mount_identity_selects_its_record_not_the_rebound_named_mount() {
    let held = "36 35 8:1 / /retained rw - btrfs /dev/held rw\n";
    let replacement = "37 35 8:2 / /configured rw - btrfs /dev/replacement rw\n";
    for table in [
        format!("{held}{replacement}"),
        format!("{replacement}{held}"),
    ] {
        let selected = resolve_id(&table, 36).unwrap();
        assert_eq!(selected.source, Path::new("/dev/held"));
        assert_eq!(selected.kind, "btrfs");
        assert_eq!(
            resolve(&table, Path::new("/configured")).unwrap().source,
            Path::new("/dev/replacement")
        );
    }
}

#[test]
fn absent_held_mount_cannot_fall_back_to_an_available_named_mount() {
    let table = record("/configured", "/dev/replacement");
    assert_eq!(
        resolve_id(&table, 37).unwrap_err().code(),
        storage_contract::ErrorCode::DiskIdentityMismatch
    );
    let overflow = table.replacen("36 ", "18446744073709551616 ", 1);
    for error in [
        resolve_id(&overflow, 36).unwrap_err(),
        resolve(&overflow, Path::new("/configured")).unwrap_err(),
    ] {
        use std::error::Error as _;

        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::DiskIdentityMismatch
        );
        assert_eq!(error.message(), "invalid mountinfo mount identity");
        assert_eq!(
            error.to_string(),
            "DISK_IDENTITY_MISMATCH: invalid mountinfo mount identity"
        );
        assert!(!error.is_retryable());
        assert!(error.published_result().is_none());
        let contextual_source = error.source().unwrap();
        assert!(contextual_source.is::<Box<Error>>());
        let parse_source = contextual_source
            .source()
            .unwrap()
            .downcast_ref::<std::num::ParseIntError>()
            .unwrap();
        assert_eq!(parse_source.kind(), &std::num::IntErrorKind::PosOverflow);
        assert!(parse_source.source().is_none());
    }
}
