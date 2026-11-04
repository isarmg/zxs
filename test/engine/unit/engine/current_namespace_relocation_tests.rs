use super::*;

fn file(path: &str, version: &str) -> Entry {
    Entry {
        entry_id: "entry-shared".into(),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: "file-shared".into(),
        version_id: version.into(),
        size: 11,
    }
}

#[test]
fn membership_registry_relocation_consumes_only_the_moved_visible_alias() {
    let before = [file("a", "v1"), file("b", "v1")];
    let after = [file("a", "v1"), file("c", "v1")];
    let admitted = VerifiedEntryChanges::between(&before, &after);
    assert!(!admitted.preserves_occurrence(&after[0]));
    assert!(admitted.preserves_occurrence(&after[1]));
    assert!(admitted.vacated("b"));
    assert!(!admitted.vacated("a"));

    let copy = [file("a", "v1"), file("b", "v1"), file("c", "v1")];
    assert!(!VerifiedEntryChanges::between(&before, &copy).preserves_occurrence(&copy[2]));
    let duplicate = [file("c", "v1"), file("d", "v1"), file("e", "v1")];
    let admitted = VerifiedEntryChanges::between(&before, &duplicate);
    assert!(admitted.preserves_occurrence(&duplicate[0]));
    assert!(admitted.preserves_occurrence(&duplicate[1]));
    assert!(!admitted.preserves_occurrence(&duplicate[2]));
}

#[test]
fn membership_registry_relocation_cannot_exchange_alias_versions_or_sizes() {
    let before = [file("a", "v1"), file("b", "v2")];
    let after = [file("a", "v1"), file("c", "v1")];
    assert!(!VerifiedEntryChanges::between(&before, &after).preserves_occurrence(&after[1]));
    let after = [file("a", "v1"), file("c", "v2")];
    assert!(VerifiedEntryChanges::between(&before, &after).preserves_occurrence(&after[1]));
    let mut resized = after.clone();
    resized[1].size += 1;
    assert!(!VerifiedEntryChanges::between(&before, &resized).preserves_occurrence(&resized[1]));
}

#[test]
fn membership_registry_case_only_relocation_preserves_the_complete_body() {
    let before = [file("Straße/Visible.txt", "v1")];
    let after = [file("STRASSE/VISIBLE.TXT", "v1")];
    assert_eq!(before[0].lookup_key(), after[0].lookup_key());
    assert!(VerifiedEntryChanges::between(&before, &after).preserves_occurrence(&after[0]));
    let changed = [file("STRASSE/VISIBLE.TXT", "v2")];
    assert!(!VerifiedEntryChanges::between(&before, &changed).preserves_occurrence(&changed[0]));
}

#[test]
fn membership_registry_relocation_does_not_reuse_an_updated_source_alias() {
    let before = [file("a", "v1")];
    let after = [file("a", "v2"), file("moved", "v1")];
    assert!(!VerifiedEntryChanges::between(&before, &after).preserves_occurrence(&after[1]));
}
