use super::*;

fn editor() -> Editor {
    let basis = Basis {
        epoch: 1,
        principal: "alice".parse().unwrap(),
        generation: 2,
        slot: "P01".into(),
        directory: String::new(),
        repository: format!("repo_{:032x}", 1),
        head: format!("commit_{:032x}", 1),
        entry: format!("entry_{:032x}", 1),
        source: "source.txt".into(),
        display_path: "source.txt".into(),
    };
    let request = ObjectMoveRequest {
        source_repository_id: HistoryRepositoryId::parse(&basis.repository).unwrap(),
        source_entry_id: NamespaceEntryId::parse(&basis.entry).unwrap(),
        source_path: name_policy::LookupPath::parse(&basis.source).unwrap(),
        target_repository_id: None,
        target_slot_id: "P02".into(),
        target_path: name_policy::PortablePath::parse("target.txt").unwrap(),
        replace: false,
        target_entry_id: None,
    };
    Editor {
        draft: RwSignal::new(Some(basis.clone())),
        target_slot: RwSignal::new("P02".into()),
        destination: RwSignal::new("target.txt".into()),
        review: RwSignal::new(Some(Review {
            basis,
            destination: "target.txt".into(),
            request,
        })),
        folders: RwSignal::new(None),
        error: RwSignal::new(None),
        reading: RwSignal::new(false),
        generation: RwSignal::new(3),
        cancellation: RwSignal::new(None),
        completion: RwSignal::new(None),
    }
}

#[test]
fn a_new_review_of_the_same_target_does_not_reactivate_the_old_button() {
    Owner::new().with(|| {
        let editor = editor();
        let generation = editor.generation.get_untracked();
        let review = editor.review.get_untracked();
        assert!(editor.owns_generation(generation));
        editor.clear_review();
        assert!(!editor.owns_generation(generation));
        editor.review.set(review);
        assert!(!editor.owns_generation(generation));
        assert!(editor.owns_generation(editor.generation.get_untracked()));
    });
}

#[test]
fn cancelling_the_editor_invalidates_its_review_generation() {
    Owner::new().with(|| {
        let editor = editor();
        let generation = editor.generation.get_untracked();
        editor.draft.set(None);
        editor.clear_review();
        assert!(!editor.owns_generation(generation));
        assert!(editor.review.get_untracked().is_none());
    });
}
