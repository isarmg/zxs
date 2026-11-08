//! Current HTTP save-intent writes and recovery. No import/publication claim.
use super::fixture::{Fixture, bytes, request};
use futures::StreamExt;
use http_routing::{
    body::{Body, Bytes},
    http::StatusCode,
};
use json_codec::{Value, json};
use std::{fs, time::Duration};
use storage_contract::{
    OperationId, storage_foreign::ForeignExecutionCapability, storage_foreign_intent::*,
    storage_foreign_plan::*,
};
use tokio::sync::oneshot;

const PREVIEW: &str = "/api/admin/storage/foreign/execution-intents/preview";
const SAVE: &str = "/api/admin/storage/foreign/execution-intents/save";
const STATUS: &str = "/api/admin/storage/foreign/execution-intents/status";
const INTENTS: &str = "metadata/foreign-import-intents";

async fn reviewed(f: &Fixture) -> ForeignIntentRequest {
    let target_slot = f.activate_target().await;
    let source = f.preview(&f.selected().await).await;
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
                .enumerate()
                .map(|(i, h)| (h.group_id, format!("history-{i}")))
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
    let plan = ForeignPlanSelection {
        operation_id: request.operation_id,
        review_digest: preview.review_digest.clone(),
    };
    f.json(
        "/api/admin/storage/foreign/plans/save",
        json_codec::to_value(ForeignPlanSave {
            preview,
            acknowledge_plan_only: true,
        })
        .unwrap(),
    )
    .await;
    ForeignIntentRequest {
        operation_id: OperationId::generate().unwrap(),
        plan,
    }
}
async fn preview(f: &Fixture, request: &ForeignIntentRequest) -> ForeignIntentPreview {
    json_codec::from_value(
        f.json(PREVIEW, json_codec::to_value(request).unwrap())
            .await,
    )
    .unwrap()
}
async fn save(f: &Fixture, preview: &ForeignIntentPreview) -> ForeignIntentStatus {
    json_codec::from_value(
        f.json(
            SAVE,
            json_codec::to_value(ForeignIntentSave {
                preview: preview.clone(),
                confirm_save_intent: true,
            })
            .unwrap(),
        )
        .await,
    )
    .unwrap()
}

#[tokio::test]
async fn multi_origin_intent_cold_replay_needs_no_source_and_publishes_nothing() {
    let mut f = Fixture::multi_origin().await;
    let request = reviewed(&f).await;
    let preview = preview(&f, &request).await;
    let source_before = bytes(&f.source);
    let target_before = bytes(&f.target);
    assert_eq!(preview.plan.source.histories.len(), 2);
    let local = preview
        .plan
        .source
        .histories
        .iter()
        .find(|h| h.repository_name == "foreign-history")
        .unwrap();
    let unavailable = preview
        .plan
        .source
        .histories
        .iter()
        .find(|h| h.repository_name == "other-origin")
        .unwrap();
    assert_ne!(local.origin, unavailable.origin);
    assert_eq!(local.versions, 1);
    assert_eq!(local.verified_local_versions, 1);
    assert!(local.temporarily_unavailable_versions.is_empty());
    assert_eq!(unavailable.versions, 1);
    assert_eq!(unavailable.verified_local_versions, 0);
    assert_eq!(unavailable.temporarily_unavailable_versions.len(), 1);
    assert_eq!(preview.effect, ForeignIntentEffect::SaveExecutionIntent);
    assert_eq!(preview.execution, ForeignExecutionCapability::Unavailable);
    let denied = ForeignIntentSave {
        preview: preview.clone(),
        confirm_save_intent: false,
    };
    assert_eq!(
        f.send(
            f.browser("POST", SAVE),
            json_codec::to_value(denied).unwrap()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(bytes(&f.target), target_before);
    let saved = save(&f, &preview).await;
    assert_eq!(saved.stage, ForeignIntentStage::IntentSaved);
    assert_eq!(saved.execution, ForeignExecutionCapability::Unavailable);
    assert_eq!(saved.histories, 2);
    assert_eq!(saved.temporarily_unavailable_versions, 1);
    let target_after = bytes(&f.target);
    let mut added = 0;
    for (path, body) in &target_after {
        if path.starts_with(INTENTS) {
            added += 1;
        } else {
            assert_eq!(
                target_before.get(path),
                Some(body),
                "changed active-pool bytes: {path:?}"
            );
        }
    }
    assert_eq!(added, 1);
    assert_eq!(target_after.len(), target_before.len() + 1);
    assert_eq!(bytes(&f.source), source_before);
    let retained = bytes(&f.target.join(INTENTS));
    let detached = f.discovery.parent().unwrap().join("detached-source");
    fs::rename(&f.source, &detached).unwrap();
    f.reopen().await;
    let after_reopen = bytes(&f.target);
    let status: ForeignIntentStatus = json_codec::from_value(
        f.json(STATUS, json_codec::to_value(&saved.address).unwrap())
            .await,
    )
    .unwrap();
    assert_eq!(status, saved);
    assert_eq!(save(&f, &preview).await, saved);
    assert_eq!(bytes(&f.target.join(INTENTS)), retained);
    assert_eq!(bytes(&detached), source_before);
    // A retained target-local intent never manufactures an execution capability.
    let (status, error) = f
        .send(
            f.browser("POST", "/api/admin/storage/foreign/plans/execute"),
            json_codec::to_value(&request.plan).unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error:?}");
    assert_eq!(error["error"]["code"], "FORMAT_UNSUPPORTED");
    assert_eq!(bytes(&f.target), after_reopen);
    f.finish().await;
}

#[tokio::test]
async fn intent_save_rechecks_session_after_streamed_request_body() {
    let f = Fixture::new().await;
    let request = reviewed(&f).await;
    let preview = preview(&f, &request).await;
    let source_before = bytes(&f.source);
    let target_before = bytes(&f.target);
    let body = json_codec::to_vec(&ForeignIntentSave {
        preview,
        confirm_save_intent: true,
    })
    .unwrap();
    let (entered, waiting) = oneshot::channel();
    let (resume, resumed) = oneshot::channel();
    let prefix = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"{"))]);
    let remainder = futures::stream::once(async move {
        entered.send(()).unwrap();
        resumed.await.unwrap();
        Ok::<_, std::io::Error>(Bytes::copy_from_slice(&body[1..]))
    });
    let request = f
        .browser("POST", SAVE)
        .body(Body::from_stream(prefix.chain(remainder)))
        .unwrap();
    let (status, error) = {
        let pending = f.raw(request);
        tokio::pin!(pending);
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = waiting => result.unwrap(),
                result = &mut pending => panic!("save completed before body continuation: {result:?}"),
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
    assert_eq!(bytes(&f.source), source_before);
    assert_eq!(bytes(&f.target), target_before);
    f.finish().await;
}

#[tokio::test]
async fn intent_save_binds_complete_review_actor_and_effect() {
    let f = Fixture::new().await;
    let request_value = reviewed(&f).await;
    let preview = preview(&f, &request_value).await;
    let original = json_codec::to_value(ForeignIntentSave {
        preview: preview.clone(),
        confirm_save_intent: true,
    })
    .unwrap();
    let source_before = bytes(&f.source);
    let target_before = bytes(&f.target);
    assert_eq!(
        f.send(request("POST", SAVE), original.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send(f.cookie("POST", SAVE), original.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    for (path, value, expected) in [
        (
            "/preview/principal",
            json!("other-admin"),
            StatusCode::CONFLICT,
        ),
        (
            "/preview/intent_digest",
            json!("0".repeat(64)),
            StatusCode::CONFLICT,
        ),
        (
            "/preview/plan/request/target/parent",
            json!("changed-target"),
            StatusCode::CONFLICT,
        ),
        (
            "/preview/effect",
            json!("copy_and_publish"),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let mut forged = original.clone();
        *forged.pointer_mut(path).unwrap() = value;
        let (status, error) = f.send(f.browser("POST", SAVE), forged).await;
        assert_eq!(status, expected, "{path}: {error:?}");
    }
    let mut extra = original.clone();
    extra["copy"] = Value::Bool(true);
    assert_eq!(
        f.send(f.browser("POST", SAVE), extra).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(bytes(&f.target), target_before);
    assert_eq!(bytes(&f.source), source_before);
    f.finish().await;
}

#[tokio::test]
async fn intent_replay_binds_original_digest_without_mutating_retained_bytes() {
    // A separate real service keeps this group within the production request
    // rate budget instead of disabling or overriding recovery throttling.
    let f = Fixture::new().await;
    let request_value = reviewed(&f).await;
    let preview = preview(&f, &request_value).await;
    let original = json_codec::to_value(ForeignIntentSave {
        preview: preview.clone(),
        confirm_save_intent: true,
    })
    .unwrap();
    let source_before = bytes(&f.source);
    let saved = save(&f, &preview).await;
    let retained = bytes(&f.target);
    let mut wrong = saved.address.clone();
    wrong.intent_digest = "0".repeat(64);
    assert_eq!(
        f.send(
            f.browser("POST", STATUS),
            json_codec::to_value(wrong).unwrap()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let mut conflict = original;
    conflict["preview"]["intent_digest"] = json!("0".repeat(64));
    assert_eq!(
        f.send(f.browser("POST", SAVE), conflict).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(save(&f, &preview).await, saved);
    assert_eq!(bytes(&f.target), retained);
    assert_eq!(bytes(&f.source), source_before);
    f.finish().await;
}
