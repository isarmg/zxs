use super::*;

#[test]
fn derived_ids_are_stable_scoped_and_reject_existing_target_collisions() {
    let scope = "original-current-state-and-source";
    let original = "file_11111111111111111111111111111111";
    let one = assign(
        scope,
        "file",
        std::iter::once(original),
        &mut BTreeSet::new(),
    )
    .unwrap();
    let again = assign(
        scope,
        "file",
        std::iter::once(original),
        &mut BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(one, again);
    assert_ne!(one[original], original);
    let other = assign(
        "different-current-state",
        "file",
        std::iter::once(original),
        &mut BTreeSet::new(),
    )
    .unwrap();
    assert_ne!(one, other);
    let mut occupied = BTreeSet::from([one[original].clone()]);
    assert!(assign(scope, "file", std::iter::once(original), &mut occupied).is_err());
}
#[test]
fn audit_identity_domains_do_not_become_local_operation_ids_or_accounts() {
    let original = "op_11111111111111111111111111111111";
    let operation = assign(
        "scope",
        "foreignop",
        std::iter::once(original),
        &mut BTreeSet::new(),
    )
    .unwrap();
    assert!(operation[original].parse::<OperationId>().is_err());
    let actor = assign(
        "scope",
        "foreignactor",
        std::iter::once("admin"),
        &mut BTreeSet::new(),
    )
    .unwrap();
    assert_ne!(actor["admin"], "admin");
    assert!(actor["admin"].starts_with("foreignactor_"));
}
