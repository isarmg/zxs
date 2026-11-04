use super::*;

#[test]
fn url_normalization_cannot_escape_the_api_boundary() {
    for value in [
        "/api/../private",
        "/api/%2e%2e/private",
        "/api/.%2E/private",
        "/api/%2e/private",
        "/api/a%2Fb",
        "/api/a%5Cb",
        "/api/a\\b",
        "/api/\t../private",
        "/api/a\n",
        "/api/a#fragment",
        "//example.org/api/a",
        "https://example.org/api/a",
        "/private",
        "/v1/repositories",
        "/api",
    ] {
        assert!(validate_api_path(value).is_err(), "accepted {value:?}");
    }
}

#[test]
fn query_values_and_ordinary_encoded_names_remain_data() {
    for value in [
        "/api/system/status",
        "/api/repositories/repo_00000000000000000000000000000000/directories/entry_00000000000000000000000000000000/history?limit=64",
        "/api/repositories/repo_00000000000000000000000000000000/directories/entry_00000000000000000000000000000000/history/branches?limit=64",
        "/api/files/%E6%96%87%E4%BB%B6",
        "/api/files?path=a/../b&name=%2f%2e%5c",
        "/api/files?path=./relative",
        "/api/files/a..b",
        "/api/files/%252e%252e",
    ] {
        assert!(validate_api_path(value).is_ok(), "rejected {value:?}");
    }
}
