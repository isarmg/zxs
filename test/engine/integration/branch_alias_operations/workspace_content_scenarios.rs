//! Content-only writes retain independently selected and frozen aliases.
use super::*;

#[test]
fn content_only_inherited_alias_write_preserves_other_selected_and_frozen_contexts() {
    let mut p = Pool::new();
    let (selected_a, selected_b, original) = selected_aliases(&mut p);
    let frozen = p.register("a", "frozen source");
    p.select("a", "n");
    let frozen_before = p.branch(&frozen);
    let selected_a_before = p.branch(&selected_a);
    let selected_b_before = p.branch(&selected_b);
    let a = p.current_entry("a/report.txt");
    let b = p.current_entry("b/report.txt");
    assert_eq!(a.file_id, b.file_id);
    let a_context = p
        .engine
        .file_context(&p.repo.repository_id, "a/report.txt")
        .unwrap();
    let b_context = p
        .engine
        .file_context(&p.repo.repository_id, "b/report.txt")
        .unwrap();
    assert!(a_context.inherited_from.is_some());
    assert!(b_context.inherited_from.is_none());
    assert_ne!(a_context.branch_id, b_context.branch_id);
    let physical_path = p.engine.state().file_locations[&original.file_id].clone();
    let physical_name = p.engine.state().file_names[&original.file_id].clone();
    let b_file_branch = p.engine.state().branches[&b_context.branch_id].clone();
    let workspace = p
        .engine
        .create_workspace(&p.repo.main_branch_id, "admin")
        .unwrap();
    p.engine
        .stage_bytes(
            &workspace.workspace_id,
            "a/report.txt",
            b"alias-specific content",
        )
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_contract::OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "content-only inherited alias".into(),
        request_digest: None,
    };
    let receipt = p.engine.commit_workspace(request.clone()).unwrap();
    let written = p.current_entry("a/report.txt");
    let written_context = p
        .engine
        .file_context(&p.repo.repository_id, "a/report.txt")
        .unwrap();
    assert_eq!(written.entry_id, a.entry_id);
    assert_eq!(written.file_id, a.file_id);
    assert_eq!(written.path, a.path);
    assert_ne!(written.version_id, a.version_id);
    assert_ne!(written_context.branch_id, a_context.branch_id);
    assert!(written_context.inherited_from.is_none());
    assert_eq!(p.current_entry("b/report.txt"), b);
    assert_eq!(
        p.engine.file_context(&p.repo.repository_id, "b/report.txt"),
        Some(b_context.clone())
    );
    assert_eq!(p.branch(&selected_b), selected_b_before);
    assert_eq!(p.branch(&frozen), frozen_before);
    assert_ne!(
        p.branch(&selected_a).head_tree_id,
        selected_a_before.head_tree_id
    );
    assert_eq!(
        p.engine.state().branches[&b_context.branch_id],
        b_file_branch
    );
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        physical_path
    );
    assert_eq!(
        p.engine.state().file_names[&original.file_id],
        physical_name
    );
    assert!(
        !p.engine
            .state()
            .file_write_origins
            .contains_key(&original.file_id)
    );
    assert_eq!(p.bytes(&written.version_id), b"alias-specific content");
    assert_eq!(p.bytes(&original.version_id), b"original");
    let state = storage_engine::canonical_json(p.engine.state()).unwrap();
    assert_eq!(
        p.engine.commit_workspace(request).unwrap().result,
        receipt.result
    );
    assert_eq!(
        storage_engine::canonical_json(p.engine.state()).unwrap(),
        state
    );
    let a_key = format!("{}/a/report.txt", p.repo.repository_id);
    let b_key = format!("{}/b/report.txt", p.repo.repository_id);
    p.reopen(|engine| {
        assert_eq!(
            storage_engine::canonical_json(engine.state()).unwrap(),
            state
        );
        assert_eq!(engine.state().file_contexts[&a_key], written_context);
        assert_eq!(engine.state().file_contexts[&b_key], b_context);
        assert_eq!(
            engine.state().directory_branches[&selected_b],
            selected_b_before
        );
        assert_eq!(engine.state().directory_branches[&frozen], frozen_before);
        assert_eq!(
            engine.state().file_locations[&original.file_id],
            physical_path
        );
        assert_eq!(engine.state().file_names[&original.file_id], physical_name);
        assert!(
            !engine
                .state()
                .file_write_origins
                .contains_key(&original.file_id)
        );
        for (version, expected) in [
            (&written.version_id, b"alias-specific content".as_slice()),
            (&original.version_id, b"original".as_slice()),
        ] {
            let mut content = engine.open_content(version).unwrap();
            let mut bytes = Vec::new();
            content.file.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, expected);
        }
        assert!(engine.check().unwrap().issues.is_empty());
    });
}
