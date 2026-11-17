use super::*;

#[test]
fn automatic_request_scopes_preserve_bytes_and_bind_file_occurrence() {
    let mut input = Input {
        repository_id: "repo_11111111111111111111111111111111".parse().unwrap(),
        path: None,
        target_copies: 3,
        expected_generation: Generation::new(7),
    };
    let retained = selected_request(&input, Selection::RetainedFiles).unwrap();
    assert_eq!(
        storage_contract::canonical_json_value(&retained).unwrap(),
        concat!(
            r#"{"expected_generation":"7","path":null,"repository_id":"#,
            r#""repo_11111111111111111111111111111111","target_copies":3}"#,
        )
        .as_bytes()
    );
    assert_eq!(
        Selection::RetainedFiles.kind(),
        "backup.configure-automatic"
    );
    assert_eq!(json_codec::from_value::<Input>(retained).unwrap(), input);

    input.path = Some(PortablePath::parse("docs").unwrap());
    let entry = "entry_22222222222222222222222222222222".parse().unwrap();
    let directory = selected_request(&input, Selection::DirectoryTree(entry)).unwrap();
    assert_eq!(
        storage_contract::canonical_json_value(&directory).unwrap(),
        concat!(
            r#"{"directory_id":"entry_22222222222222222222222222222222","#,
            r#""expected_generation":"7","path":"docs","repository_id":"#,
            r#""repo_11111111111111111111111111111111","target_copies":3}"#,
        )
        .as_bytes()
    );
    assert_eq!(
        Selection::DirectoryTree(entry).kind(),
        "backup.configure-automatic-directory"
    );
    let decoded: DirectoryInput = json_codec::from_value(directory).unwrap();
    assert_eq!(decoded.directory_id, entry);
    assert_eq!(decoded.repository_id, input.repository_id);

    input.path = Some(PortablePath::parse("docs/report.txt").unwrap());
    let selected = Selection::File(FileSelection {
        entry_id: entry,
        file_id: "file_33333333333333333333333333333333".parse().unwrap(),
    });
    let original = selected_request(&input, selected).unwrap();
    assert_eq!(
        storage_contract::canonical_json_value(&original).unwrap(),
        concat!(
            r#"{"entry_id":"entry_22222222222222222222222222222222","#,
            r#""expected_generation":"7","file_id":"file_33333333333333333333333333333333","#,
            r#""path":"docs/report.txt","repository_id":"#,
            r#""repo_11111111111111111111111111111111","target_copies":3}"#,
        )
        .as_bytes()
    );
    assert_eq!(selected.kind(), "backup.configure-automatic-file");
    let decoded: FileInput = json_codec::from_value(original.clone()).unwrap();
    assert_eq!(decoded.entry_id, entry);
    assert_eq!(decoded.repository_id, input.repository_id);
    let digest = Engine::native_request_digest(selected.kind(), "admin", &original).unwrap();
    for (field, value) in [
        ("entry_id", "entry_44444444444444444444444444444444"),
        ("file_id", "file_55555555555555555555555555555555"),
        ("path", "docs/other.txt"),
        ("repository_id", "repo_66666666666666666666666666666666"),
    ] {
        let mut other = original.clone();
        other[field] = value.into();
        assert_ne!(
            Engine::native_request_digest(selected.kind(), "admin", &other).unwrap(),
            digest,
            "{field}"
        );
    }
}
