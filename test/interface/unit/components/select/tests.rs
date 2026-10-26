use super::*;
#[test]
fn keyboard_navigation_wraps_and_skips_disabled_choices() {
    let mut options = vec![
        SelectOption::new("", "Default"),
        SelectOption::new("old", "Unavailable"),
        SelectOption::new("new", "New"),
    ];
    options[1].disabled = true;
    assert_eq!(next_enabled(&options, None, true), Some(0));
    assert_eq!(next_enabled(&options, Some(0), true), Some(2));
    assert_eq!(next_enabled(&options, Some(2), true), Some(0));
    assert_eq!(next_enabled(&options, Some(0), false), Some(2));
    assert_eq!(next_enabled(&options, None, false), Some(2));
    assert_eq!(next_enabled(&options, Some(99), true), Some(0));
}
#[test]
fn empty_and_fully_disabled_lists_have_no_active_choice() {
    assert_eq!(next_enabled(&[], None, true), None);
    let options = vec![SelectOption {
        value: String::new(),
        label: "Disabled".into(),
        disabled: true,
    }];
    assert_eq!(next_enabled(&options, None, true), None);
    assert_eq!(next_enabled(&options, None, false), None);
}

#[test]
fn borrowed_label_projection_preserves_selection_and_reactive_dependencies() {
    use reactive_state::owner::Owner;
    use std::sync::Arc;

    let owner = Owner::new();
    owner.with(|| {
        let options = RwSignal::new(Vec::<SelectOption>::new());
        let value = RwSignal::new(String::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let label = Memo::new({
            let reads = Arc::clone(&reads);
            move |_| {
                reads.fetch_add(1, Ordering::Relaxed);
                options.with(|entries| {
                    entries
                        .iter()
                        .find(|item| value.with(|value| item.value == *value))
                        .map(|item| item.label.clone())
                })
            }
        });
        assert_eq!(label.get(), None);
        value.set("new".into());
        assert_eq!(label.get(), None);
        assert_eq!(
            reads.load(Ordering::Relaxed),
            1,
            "empty options do not subscribe to value"
        );

        options.set(vec![
            SelectOption::new("", "Default"),
            SelectOption {
                value: "old".into(),
                label: "Unavailable".into(),
                disabled: true,
            },
            SelectOption::new("new", "First"),
            SelectOption::new("new", "Second"),
            SelectOption::new("blank", ""),
        ]);
        assert_eq!(label.get().as_deref(), Some("First"));
        for (selected, expected) in [
            ("", Some("Default")),
            ("old", Some("Unavailable")),
            ("blank", Some("")),
            ("missing", None),
        ] {
            value.set(selected.into());
            assert_eq!(label.get().as_deref(), expected);
        }
        value.set("new".into());
        options.update(|entries| entries[2].label = "Updated".into());
        assert_eq!(label.get().as_deref(), Some("Updated"));
        options.set(vec![]);
        assert_eq!(label.get(), None);
        let before = reads.load(Ordering::Relaxed);
        value.set("old".into());
        assert_eq!(label.get(), None);
        assert_eq!(
            reads.load(Ordering::Relaxed),
            before,
            "cleared options unsubscribe from value"
        );
    });
    owner.cleanup();
}
