use super::*;

fn branch(name: &str) -> BranchName {
    BranchName::parse(name).unwrap()
}
fn names(names: &[&str]) -> Vec<LookupName> {
    names.iter().map(|name| LookupName::parse(name).unwrap()).collect()
}

#[test]
fn names_start_at_001_and_share_the_real_parent_namespace() {
    let first = allocate(&branch("main"), [], 255).unwrap();
    assert_eq!(first.name().as_str(), "worktree-main-001");
    assert_eq!(first.ordinal(), 1);
    let next = allocate(&branch("main"), names(&[
        "worktree-main-001", "WORKTREE-MAIN-002", "worktree-other-003",
    ]), 255).unwrap();
    assert_eq!(next.name().as_str(), "worktree-main-003");
}

#[test]
fn a_durable_claim_prevents_two_live_instances_getting_the_same_name() {
    let first = allocate(&branch("topic"), [], 255).unwrap();
    let claimed = LookupName::from(first.name());
    let second = allocate(&branch("topic"), [claimed], 255).unwrap();
    assert_eq!(second.name().as_str(), "worktree-topic-002");
}

#[test]
fn unicode_collision_keys_do_not_change_the_selected_branch_display_spelling() {
    let next = allocate(&branch("Straße"), names(&["WORKTREE-STRASSE-001"]), 255).unwrap();
    assert_eq!(next.name().as_str(), "worktree-Straße-002");
}

#[test]
fn exhaustion_preserves_every_occupied_name_and_never_wraps_or_deletes() {
    let occupied = (1..=999).map(|ordinal| {
        LookupName::parse(&format!("worktree-main-{ordinal:03}")).unwrap()
    }).collect::<Vec<_>>();
    assert!(matches!(allocate(&branch("main"), occupied.clone(), 255), Err(NameError::Exhausted)));
    let other = allocate(&branch("other"), occupied, 255).unwrap();
    assert_eq!(other.ordinal(), 1);
}

#[test]
fn deleted_name_can_be_reused_but_restore_must_claim_another_free_name() {
    let replacement = allocate(&branch("main"), [], 255).unwrap();
    let restoration = allocate(&branch("main"), [LookupName::from(replacement.name())], 255)
        .unwrap();
    assert_eq!(replacement.ordinal(), 1);
    assert_eq!(restoration.ordinal(), 2);
}

#[test]
fn target_component_limit_counts_utf8_bytes_not_characters() {
    let full = "worktree-中文-001";
    assert!(matches!(allocate(&branch("中文"), [], full.len() - 1), Err(NameError::TooLong)));
    assert_eq!(allocate(&branch("中文"), [], full.len()).unwrap().name().as_str(), full);
}

#[test]
fn similar_ordinary_names_do_not_consume_other_sequence_numbers() {
    let first = allocate(&branch("main"), names(&[
        "worktree-main-000", "worktree-main-1", "worktree-main-0001", "worktree-main-999x",
    ]), 255).unwrap();
    assert_eq!(first.ordinal(), 1);
}
