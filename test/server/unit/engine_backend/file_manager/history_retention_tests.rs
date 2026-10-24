use super::*;

fn project_history_sources(
    sources: &[(u64, Vec<String>)],
    skip_older: bool,
) -> (BTreeMap<(Reverse<u64>, String), Value>, usize) {
    let mut events = BTreeMap::new();
    let mut projected = 0;
    for (source, (sequence, ids)) in sources.iter().enumerate() {
        if skip_older && !history_sequence_may_fit(&events, *sequence) {
            continue;
        }
        for id in ids {
            projected += 1;
            record_event(
                &mut events,
                json!({"sequence":sequence.to_string(),"event_id":id,"source":source}),
            );
        }
    }
    (events, projected)
}

#[test]
fn history_sequence_cutoff_waits_for_distinct_rows_and_keeps_ties() {
    let mut events = BTreeMap::new();
    assert!(history_sequence_may_fit(&events, 0));
    for _ in 0..250 {
        record_event(&mut events, json!({"sequence":"5","event_id":"duplicate"}));
    }
    assert_eq!(events.len(), 1);
    for id in 0..198 {
        record_event(
            &mut events,
            json!({"sequence":"10","event_id":id.to_string()}),
        );
    }
    assert_eq!(events.len(), HISTORY_LIMIT - 1);
    assert!(history_sequence_may_fit(&events, 0));
    record_event(&mut events, json!({"sequence":"10","event_id":"last"}));
    assert_eq!(events.len(), HISTORY_LIMIT);
    assert!(!history_sequence_may_fit(&events, 4));
    assert!(history_sequence_may_fit(&events, 5));
    assert!(history_sequence_may_fit(&events, u64::MAX));
    record_event(
        &mut events,
        json!({"sequence":u64::MAX.to_string(),"event_id":"newest"}),
    );
    assert!(!history_sequence_may_fit(&events, 5));
    assert!(history_sequence_may_fit(&events, 10));
}

#[test]
fn history_sequence_cutoff_matches_full_projection_for_shuffled_sources() {
    // Empty recent sources and repeated keys must never count as a full page.
    let mut sources = (0..250).map(|_| (900, Vec::new())).collect::<Vec<_>>();
    sources.push((1, vec!["duplicate".into(); 250]));
    // More than one page at the same sequence, split across multiple sources.
    // IDs intentionally do not sort in numeric order.
    for group in 0..20 {
        sources.push((
            500,
            (group * 20..(group + 1) * 20)
                .rev()
                .map(|id| format!("opaque-{id}"))
                .collect(),
        ));
    }
    sources.extend(
        (0..300).map(|sequence| (sequence, vec![format!("older-{sequence}"), "shared".into()])),
    );
    sources.push((500, vec!["opaque-0".into(), "opaque-10".into()]));
    sources.push((u64::MAX, vec!["maximum".into()]));
    sources.push((u64::MAX, vec!["maximum".into(), "another-maximum".into()]));

    for seed in 0_u64..16 {
        let mut shuffled = sources.clone();
        if seed != 0 {
            let mut random = seed;
            for index in (1..shuffled.len()).rev() {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                shuffled.swap(index, (random % (index as u64 + 1)) as usize);
            }
        }
        let (expected, _) = project_history_sources(&shuffled, false);
        let (actual, _) = project_history_sources(&shuffled, true);
        assert_eq!(actual, expected, "shuffle seed {seed}");
        assert_eq!(actual.len(), HISTORY_LIMIT);
        assert_eq!(actual.last_key_value().unwrap().0.0, Reverse(500));
    }
}

#[test]
fn history_sequence_cutoff_skips_older_projection_without_stopping_the_scan() {
    let mut sources = vec![(
        1000,
        (0..HISTORY_LIMIT)
            .map(|id| format!("recent-{id}"))
            .collect(),
    )];
    sources.extend((0..1000).map(|sequence| {
        (
            sequence,
            (0..10).map(|id| format!("older-{sequence}-{id}")).collect(),
        )
    }));
    sources.push((2000, vec!["newest-after-older-sources".into()]));
    let (expected, full_projection) = project_history_sources(&sources, false);
    let (actual, limited_projection) = project_history_sources(&sources, true);
    assert_eq!(actual, expected);
    assert_eq!(full_projection, HISTORY_LIMIT + 10_000 + 1);
    assert_eq!(limited_projection, HISTORY_LIMIT + 1);
    assert_eq!(actual.first_key_value().unwrap().0.0, Reverse(2000));
}
