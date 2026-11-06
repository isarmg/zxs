//! Real management HTTP admission and non-executable foreign review boundaries.
//! No SQLite prototype, synthetic publication issuer or import writer is used.
#[path = "foreign_admission/fixture.rs"]
mod fixture;
use fixture::{Fixture, PREVIEW, bytes, request};
use futures::StreamExt;
use http_routing::{
    body::{Body, Bytes},
    http::{Request, StatusCode},
};
use json_codec::{Value, json};
use std::time::Duration;
use storage_contract::{
    OperationId,
    storage_foreign::{ForeignDiskPreviewRequest, ForeignExecutionCapability},
    storage_foreign_plan::*,
};
use tokio::sync::oneshot;

fn source_request(source: storage_contract::storage_directories::DirectorySource) -> Value {
    json_codec::to_value(ForeignDiskPreviewRequest { source }).unwrap()
}

#[tokio::test]
async fn foreign_preview_requires_cookie_csrf_and_same_origin() {
    let f = Fixture::new().await;
    let selected = f.selected().await;
    let before = bytes(&f.source);
    let body = source_request(selected.clone());
    assert_eq!(
        f.send(request("POST", PREVIEW), body.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send(f.cookie("POST", PREVIEW), body.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    let cross_origin = Request::builder()
        .method("POST")
        .uri(PREVIEW)
        .header("host", "127.0.0.1:7400")
        .header("origin", "http://untrusted.invalid")
        .header("sec-fetch-site", "cross-site")
        .header("content-type", "application/json")
        .header("cookie", format!("zxs_session={}", f.token))
        .header("x-csrf-token", &f.csrf);
    assert_eq!(f.send(cross_origin, body).await.0, StatusCode::FORBIDDEN);
    let preview = f.preview(&selected).await;
    assert_eq!(preview.execution, ForeignExecutionCapability::Unavailable);
    assert_eq!(preview.histories.len(), 1);
    assert_eq!(preview.histories[0].verified_local_versions, 1);
    assert_eq!(bytes(&f.source), before);
    assert!(bytes(&f.target).is_empty());
    f.finish().await;
}

#[tokio::test]
async fn foreign_preview_rechecks_session_after_streamed_request_body() {
    let f = Fixture::new().await;
    let selected = f.selected().await;
    assert_eq!(
        f.preview(&selected).await.execution,
        ForeignExecutionCapability::Unavailable
    );
    let before = bytes(&f.source);
    let body = json_codec::to_vec(&source_request(selected)).unwrap();
    let (entered, waiting) = oneshot::channel();
    let (resume, resumed) = oneshot::channel();
    let prefix = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"{"))]);
    let remainder = futures::stream::once(async move {
        entered.send(()).unwrap();
        resumed.await.unwrap();
        Ok::<_, std::io::Error>(Bytes::copy_from_slice(&body[1..]))
    });
    let request = f
        .browser("POST", PREVIEW)
        .body(Body::from_stream(prefix.chain(remainder)))
        .unwrap();
    let (status, error) = {
        let pending = f.raw(request);
        tokio::pin!(pending);
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = waiting => result.unwrap(),
                result = &mut pending => panic!("preview finished before body continuation: {result:?}"),
            }
        }).await.unwrap();
        assert_eq!(
            f.json("/api/auth/logout", json!({})).await["logged_out"],
            true
        );
        resume.send(()).unwrap();
        pending.await
    };
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error:?}");
    assert_eq!(error["error"]["code"], "UNAUTHORIZED");
    assert_eq!(bytes(&f.source), before);
    assert!(bytes(&f.target).is_empty());
    // The test observes rejection and unchanged data, not source-syscall counts.
    f.finish().await;
}

#[tokio::test]
async fn foreign_preview_rejects_stale_sources_and_unreviewed_write_fields() {
    let f = Fixture::new().await;
    let selected = f.selected().await;
    let before = bytes(&f.source);
    let mut stale = selected.clone();
    stale.identity.inode += 1;
    let (status, error) = f
        .send(f.browser("POST", PREVIEW), source_request(stale))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error:?}");
    assert_eq!(error["error"]["code"], "DISK_CHANGED");
    let mut outside = selected.clone();
    outside.path = f
        .discovery
        .parent()
        .unwrap()
        .join("foreign-system")
        .to_str()
        .unwrap()
        .into();
    assert_eq!(
        f.send(f.browser("POST", PREVIEW), source_request(outside))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut forged = source_request(selected.clone());
    forged["execute"] = json!(true);
    forged["publication_proof"] = json!({"checkpoint":"0".repeat(64),"verified":true});
    assert_eq!(
        f.send(f.browser("POST", PREVIEW), forged).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.send(
            f.browser("POST", &format!("{PREVIEW}?execute=true")),
            source_request(selected.clone())
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.preview(&selected).await.execution,
        ForeignExecutionCapability::Unavailable
    );
    assert_eq!(bytes(&f.source), before);
    assert!(bytes(&f.target).is_empty());
    f.finish().await;
}

#[tokio::test]
async fn saved_and_revalidated_foreign_review_never_grants_import_execution() {
    let f = Fixture::new().await;
    let target_slot = f.activate_target().await;
    let source = f.preview(&f.selected().await).await;
    let source_before = bytes(&f.source);
    let target_before = bytes(&f.target);
    let request = ForeignPlanRequest {
        operation_id: OperationId::generate().unwrap(),
        source: source.source,
        source_review_digest: source.review_digest,
        target: ForeignPlanTarget {
            slot_id: target_slot,
            parent: None,
            history_names: source
                .histories
                .into_iter()
                .map(|history| (history.group_id, "import-review".into()))
                .collect(),
        },
    };
    let preview: ForeignPlanPreview = json_codec::from_value(
        f.json(
            "/api/admin/storage/foreign/plans/preview",
            json_codec::to_value(&request).unwrap(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(preview.execution, ForeignExecutionCapability::Unavailable);
    assert_eq!(
        preview.blocker,
        ForeignPlanBlocker::PortableImportWriterUnavailable
    );
    let save = ForeignPlanSave {
        preview: preview.clone(),
        acknowledge_plan_only: true,
    };
    let saved: ForeignPlanStatus = json_codec::from_value(
        f.json(
            "/api/admin/storage/foreign/plans/save",
            json_codec::to_value(&save).unwrap(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(saved.validation, ForeignPlanValidation::Required);
    let records = bytes(&f.management.join("foreign-reconstruction-plans"));
    assert_eq!(records.len(), 1);
    let selection = ForeignPlanSelection {
        operation_id: request.operation_id,
        review_digest: preview.review_digest.clone(),
    };
    let checked: ForeignPlanStatus = json_codec::from_value(
        f.json(
            "/api/admin/storage/foreign/plans/revalidate",
            json_codec::to_value(&selection).unwrap(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(
        checked.validation,
        ForeignPlanValidation::MatchedAtObservation
    );
    assert_eq!(
        checked.preview.execution,
        ForeignExecutionCapability::Unavailable
    );
    let (status, error) = f
        .send(
            f.browser("POST", "/api/admin/storage/foreign/plans/execute"),
            json_codec::to_value(&selection).unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error:?}");
    assert_eq!(error["error"]["code"], "FORMAT_UNSUPPORTED");
    let mut upgraded = json_codec::to_value(save).unwrap();
    upgraded["preview"]["execution"] = json!("ready");
    assert_eq!(
        f.send(
            f.browser("POST", "/api/admin/storage/foreign/plans/save"),
            upgraded
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, stored) = f
        .raw(
            f.browser(
                "GET",
                &format!(
                    "/api/admin/storage/foreign/plans/status?operation_id={}",
                    request.operation_id
                ),
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{stored:?}");
    assert_eq!(
        json_codec::from_value::<ForeignPlanStatus>(stored).unwrap(),
        saved
    );
    assert_eq!(bytes(&f.source), source_before);
    assert_eq!(bytes(&f.target), target_before);
    assert_eq!(
        bytes(&f.management.join("foreign-reconstruction-plans")),
        records
    );
    f.finish().await;
}
