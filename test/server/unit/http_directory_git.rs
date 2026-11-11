use super::*;

#[test]
fn status_input_retains_repository_root_and_original_lookup_path() {
    let repository = "repo_00000000000000000000000000000000";
    for raw in ["", "Cafe\u{301}/Child"] {
        let input = StatusInput::parse(repository, StatusQuery { path: raw.into() }).unwrap();
        assert_eq!(input.repository.to_string(), repository);
        assert_eq!(
            input.path.as_ref().map(name_policy::LookupPath::as_str),
            if raw.is_empty() { None } else { Some(raw) }
        );
        if let Some(path) = input.path {
            assert_eq!(path.key(), "café/child");
        }
    }
    for (repository, path) in [
        ("repo_0000000000000000000000000000000A", ""),
        ("entry_00000000000000000000000000000000", ""),
        (repository, "../bad"),
        (repository, "/absolute"),
    ] {
        let error = StatusInput::parse(repository, StatusQuery { path: path.into() })
            .err()
            .unwrap();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(std::error::Error::source(&error).is_some());
    }
}
