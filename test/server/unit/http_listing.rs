use super::*;
use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
};
use json_codec::{Value, json};
use service_stack::ServiceExt;

const ROUTE: &str = "/api/file-manager/disks/P01/entries";

async fn inspect(ListingInput(request): ListingInput) -> Result<Json<Value>, ApiError> {
    let request = request?;
    Ok(Json(json!({
        "slot": request.slot_id(),
        "path": request.path(),
        "key": request.path.as_ref().map(name_policy::LookupPath::key),
        "limit": request.limit(),
        "query": request.query(),
        "selected": request.selected(),
        "preview": request.preview(),
        "cursor": request.cursor().map(|cursor| (
            cursor.snapshot.as_str(),
            cursor.request.as_str(),
            cursor.backward,
        )),
    })))
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Body,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn listing_input_retains_defaults_lookup_spelling_and_typed_selection() {
    let app = Router::new().route("/api/file-manager/disks/{slot}/entries", get(inspect));
    let (status, value) = send(&app, "GET", ROUTE, None, Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value,
        json!({
            "slot": "P01",
            "path": "",
            "key": null,
            "limit": 200,
            "query": "",
            "selected": null,
            "preview": null,
            "cursor": null,
        })
    );
    let uri = format!(
        "{ROUTE}?path=Cafe%CC%81&limit=500&query=ZETA&selected_path=CAF%C3%89/File\
         &selected_entry_id=entry&directory_branch_id=branch&expected_tree_id=tree"
    );
    let (status, value) = send(&app, "GET", &uri, None, Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value,
        json!({
            "slot": "P01",
            "path": "Cafe\u{301}",
            "key": "café",
            "limit": 500,
            "query": "zeta",
            "selected": ["café/file", "entry"],
            "preview": ["branch", "tree"],
            "cursor": null,
        })
    );
    // Slot syntax remains the backend's responsibility at this endpoint.
    let uri = "/api/file-manager/disks/not-a-slot/entries?query=%FF";
    let (status, value) = send(&app, "GET", uri, None, Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["slot"], "not-a-slot");
    assert_eq!(value["query"], "\u{fffd}");
}

#[tokio::test]
async fn listing_input_preserves_decoded_cursor_and_strict_query_failures() {
    let app = Router::new().route("/api/file-manager/disks/{slot}/entries", get(inspect));
    let cursor = crate::backend_listing::ListingCursor {
        snapshot: "snapshot".into(),
        request: "request".into(),
        backward: true,
        key: crate::backend_listing::SortKey {
            file: true,
            folded: "file".into(),
            normalized: "file".into(),
            name: "File".into(),
            path: "File".into(),
            identity: "entry".into(),
        },
    };
    let cursor: String = json_codec::to_vec(&cursor)
        .unwrap()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let uri = format!("{ROUTE}?cursor={cursor}");
    let (status, value) = send(&app, "GET", &uri, None, Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["cursor"], json!(["snapshot", "request", true]));
    for suffix in [
        "?path=a&path=b",
        "?path=a&%70ath=b",
        "?unknown=1&%75nknown=2",
        "?limit=0",
        "?limit=501",
        "?path=../bad",
        "?selected_path=item",
        "?directory_branch_id=branch",
        "?cursor=gg",
    ] {
        let uri = format!("{ROUTE}{suffix}");
        let (status, value) = send(&app, "GET", &uri, None, Body::empty()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {value}");
        assert_eq!(value["error"]["code"], "INVALID_REQUEST");
        assert!(value["error"].get("source").is_none());
    }
    let uri = "/api/file-manager/disks/%FF/entries?limit=0";
    let (status, value) = send(&app, "GET", uri, None, Body::empty()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "INVALID_REQUEST");
}

#[tokio::test]
async fn listing_route_keeps_auth_before_decoding_and_rechecks_logout() {
    let root = crate::tests::private_tempdir();
    let app = crate::tests::application(root.path());
    let malformed = "/api/file-manager/disks/%FF/entries?path=a&%70ath=b";
    let (status, value) = send(&app, "GET", malformed, None, Body::empty()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "UNAUTHORIZED");
    let (status, session) = send(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Body::from(r#"{"username":"admin","password":"a-long-test-password"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = session["token"].as_str().unwrap();
    let (status, value) = send(&app, "GET", malformed, Some(token), Body::empty()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "INVALID_REQUEST");
    let uri = format!("{ROUTE}?path=Cafe%CC%81&limit=500");
    let (status, value) = send(&app, "GET", &uri, Some(token), Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["slot_id"], "P01");
    assert_eq!(value["path"], "Cafe\u{301}");
    let (status, value) =
        send(&app, "POST", "/api/auth/logout", Some(token), Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["logged_out"], true);
    let (status, value) = send(&app, "GET", &uri, Some(token), Body::empty()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "UNAUTHORIZED");
}
