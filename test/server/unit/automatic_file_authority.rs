use super::*;

#[test]
fn batch_authority_uses_one_pass_and_the_same_first_history_for_each_real_file_and_lookahead() {
    let rows = [
        ("other", "outside"),
        ("a", "first-a"),
        ("a", "later-a"),
        ("b", "first-b"),
        ("a", "still-later-a"),
        ("c", "lookahead-c"),
        ("after-the-page", "unvisited"),
    ];
    let mut requested = BTreeSet::from(["a", "b", "c"]);
    let mut visits = 0;
    let mut admitted = Vec::new();
    first_histories(
        rows.into_iter().inspect(|_| visits += 1),
        requested.len(),
        |file| requested.remove(file),
        |history| {
            admitted.push(history.to_owned());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(visits, 6);
    assert_eq!(admitted, ["first-a", "first-b", "lookahead-c"]);
    assert!(requested.is_empty());
}

#[test]
fn denied_first_history_or_missing_omitted_file_never_yields_a_partial_authorization() {
    let rows = [
        ("visible", "original"),
        ("next", "denied"),
        ("next", "acceptable-but-later"),
    ];
    let mut needed = BTreeSet::from(["visible", "next"]);
    let mut seen = Vec::new();
    assert!(
        first_histories(
            rows.into_iter(),
            needed.len(),
            |file| needed.remove(file),
            |history| {
                seen.push(history.to_owned());
                if history == "denied" {
                    Err(ApiError::forbidden())
                } else {
                    Ok(())
                }
            }
        )
        .is_err()
    );
    assert_eq!(seen, ["original", "denied"]);
    let mut needed = BTreeSet::from(["visible", "missing-lookahead"]);
    assert!(
        first_histories(
            rows.into_iter(),
            needed.len(),
            |file| needed.remove(file),
            |_| Ok(())
        )
        .is_err()
    );
    let never = rows
        .into_iter()
        .inspect(|_| panic!("empty page must not read unrelated versions"));
    first_histories(never, 0, |_| unreachable!(), |_| unreachable!()).unwrap();
}
