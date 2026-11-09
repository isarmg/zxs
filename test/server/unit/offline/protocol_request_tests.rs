//! The old HTTP semantic cases now exercise the actual offline typed parser.
//! HTTP origin/CSRF/concurrency cases remain in helper; this adapter is not a server.
use super::*;
use http_routing::{
    Json,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use json_codec::{Value, json};
#[derive(Clone, Copy)]
struct Dispatcher;
impl Dispatcher {
    async fn oneshot(self, request: Request<Body>) -> Result<Response> {
        let operation = match request.uri().path() {
            "/api/credentials/reset-preview" => Operation::AdministratorResetPreview,
            "/api/credentials/reset" => Operation::AdministratorReset,
            "/api/credentials/reset-recover-preview" => Operation::AdministratorResetRecoverPreview,
            "/api/initialization/administrator-preview" => Operation::FirstAdministratorPreview,
            "/api/initialization/administrator-recover-preview" => {
                Operation::FirstAdministratorRecoverPreview
            }
            "/api/initialization/administrator-create" => Operation::FirstAdministratorCreate,
            "/api/initialization/storage-preview" => Operation::FirstStoragePreview,
            "/api/initialization/storage-recover-preview" => Operation::FirstStorageRecover,
            "/api/initialization/storage-create" => Operation::FirstStorageCreate,
            "/api/disks/foreign-chain-preview" => Operation::ForeignChainPreview,
            "/api/disks/foreign-chain-original" => Operation::ForeignChainOriginal,
            "/api/disks/foreign-chain-prepare" => Operation::ForeignChainPrepare,
            "/api/disks/add-preview" => Operation::DiskAdditionPreview,
            "/api/disks/add-original" => Operation::DiskAdditionOriginal,
            "/api/disks/add-commit" => Operation::DiskAdditionCommit,
            "/api/disks/registrations" => Operation::DiskRemovalInventory,
            "/api/disks/deregister-preview" => Operation::DiskRemovalPreview,
            "/api/disks/deregister-original" => Operation::DiskRemovalOriginal,
            "/api/disks/deregister-commit" => Operation::DiskRemovalCommit,
            "/api/disks/terminal-removal-inspect" => Operation::TerminalRemovalInspect,
            "/api/disks/terminal-retirement-preview" => Operation::TerminalRetirementPreview,
            "/api/disks/terminal-retirement-original" => Operation::TerminalRetirementOriginal,
            "/api/disks/terminal-retirement-commit" => Operation::TerminalRetirementCommit,
            "/api/disks/readmission-preview" => Operation::DiskReadmissionPreview,
            "/api/disks/readmission-original" => Operation::DiskReadmissionOriginal,
            "/api/disks/readmission-commit" => Operation::DiskReadmissionCommit,
            "/api/disks/terminal-chain-export" => Operation::TerminalChainExport,
            "/api/disks/terminal-chain-readmission-proposal" => {
                Operation::TerminalChainReadmissionProposal
            }
            "/api/disks/terminal-chain-target-status" => Operation::TerminalChainTargetStatus,
            "/api/disks/terminal-chain-target-synchronize" => {
                Operation::TerminalChainTargetSynchronize
            }
            "/api/disks/terminal-chain-first-prepare" => Operation::TerminalChainFirstPrepare,
            "/api/disks/terminal-chain-first-publish" => Operation::TerminalChainFirstPublish,
            "/api/disks/terminal-chain-current" => Operation::TerminalChainCurrent,
            "/api/disks/terminal-chain-activation-plan" => Operation::TerminalChainActivationPlan,
            "/api/disks/terminal-chain-io-decide" => Operation::TerminalChainIoDecide,
            "/api/disks/terminal-chain-generation-log" => Operation::TerminalChainGenerationLog,
            "/api/disks/terminal-chain-generation-select" => {
                Operation::TerminalChainGenerationSelect
            }
            "/api/disks/terminal-chain-generation-read" => Operation::TerminalChainGenerationRead,
            other => panic!("unknown test operation: {other}"),
        };
        let body = to_bytes(request.into_body(), wire::MAX_REQUEST_BYTES).await?;
        Ok(match dispatch(operation, &body) {
            Ok(bytes) => bytes.into_response(),
            Err(Failure { kind, message }) => (
                match kind {
                    FailureKind::InvalidRequest => StatusCode::BAD_REQUEST,
                    FailureKind::Conflict => StatusCode::CONFLICT,
                },
                Json(json!({"error":{"code":operation.failure_code(),"message":message}})),
            )
                .into_response(),
        })
    }
}
fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(path)
        .body(Body::from(json_codec::to_vec(&body).unwrap()))
        .unwrap()
}
async fn json_body(response: Response) -> Value {
    json_codec::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
}
#[tokio::test]
async fn helper_deregistration_input_has_no_planned_offline_or_force_lane() {
    let response = Dispatcher
        .oneshot(post(
            "/api/disks/deregister-preview",
            json!({
                "service_user":"zxs","operation_id":"op_11111111111111111111111111111111",
                "slot_id":"P01","expected_disk_id":"disk_11111111111111111111111111111111",
                "external_writers_stopped":true,"action":"planned_offline"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn terminal_inspection_has_no_operation_or_force_input() {
    let response = Dispatcher.oneshot(post(
        "/api/disks/terminal-removal-inspect",
        json!({"service_user":"zxs","slot_id":"P02","expected_disk_id":"disk_original",
            "external_writers_stopped":true,"operation_id":"op_11111111111111111111111111111111","force":true}),
    )).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn readmission_input_refuses_replacement_or_foreign_force_before_system_access() {
    for field in ["root_path", "force", "foreign_chain", "action"] {
        let mut input = json!({"service_user":"zxs","operation_id":"op_11111111111111111111111111111111",
            "slot_id":"P01","expected_disk_id":"disk_original","external_writers_stopped":true});
        input[field] = "untrusted".into();
        let response = Dispatcher
            .oneshot(post("/api/disks/readmission-preview", input))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn chain_readmission_proposal_does_not_accept_force_or_foreign_roots() {
    let path = "/api/disks/terminal-chain-readmission-proposal";
    let input = json!({"service_user":"zxs","operation_id":"op_11111111111111111111111111111111",
        "slot_id":"P01","expected_disk_id":"disk_original","external_writers_stopped":true});
    for key in [
        "force",
        "foreign_chain",
        "root_path",
        "activate",
        "checkpoint",
    ] {
        let mut changed = input.clone();
        changed[key] = "untrusted".into();
        assert_eq!(
            Dispatcher
                .oneshot(post(path, changed))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut stopped = input;
    stopped["external_writers_stopped"] = false.into();
    assert_eq!(
        Dispatcher
            .oneshot(post(path, stopped))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn chain_target_status_requires_stopped_writers_and_has_no_activate_parameter() {
    let path = "/api/disks/terminal-chain-target-status";
    let response = Dispatcher
        .oneshot(post(
            path,
            json!({"service_user":"zxs","external_writers_stopped":false}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    for field in [
        "force",
        "activate",
        "root_path",
        "foreign_chain",
        "proposal",
    ] {
        let mut input = json!({"service_user":"zxs","external_writers_stopped":true});
        input[field] = "untrusted".into();
        assert_eq!(
            Dispatcher
                .oneshot(post(path, input))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn chain_first_preparation_requires_exact_plan_and_does_not_accept_activation() {
    let path = "/api/disks/terminal-chain-first-prepare";
    for additional in [
        "force",
        "activate",
        "root_path",
        "foreign_chain",
        "core",
        "state",
        "candidate",
    ] {
        let mut value = json!({
            "service_user":"zxs", "external_writers_stopped":true,
            "confirmation":"prepare-chain-first-inputs-without-publication",
            "acknowledge_unpublished_inputs":true,
        });
        value[additional] = "untrusted".into();
        assert_eq!(
            Dispatcher
                .oneshot(post(path, value))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    // No native original plan can be synthesized from an arbitrary JSON blob.
    assert_eq!(
        Dispatcher
            .oneshot(post(
                path,
                json!({
                    "service_user":"zxs", "external_writers_stopped":true,
                    "confirmation":"prepare-chain-first-inputs-without-publication",
                    "acknowledge_unpublished_inputs":true, "plan":{},
                })
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn chain_current_input_rejects_caller_authority_or_activation_overrides() {
    for field in [
        "publication",
        "seal",
        "state",
        "registry",
        "force",
        "consume_stop",
        "new_epoch",
        "root",
        "foreign_chain",
    ] {
        let mut value = json_codec::json!({
            "service_user":"zxs", "external_writers_stopped":true, "plan":{}
        });
        value[field] = true.into();
        assert!(
            json_codec::from_value::<crate::offline::disk_removal::chain_target::CurrentInput>(
                value
            )
            .is_err()
        );
    }
}

#[test]
fn chain_activation_plan_input_has_no_caller_epoch_or_locator_override() {
    for field in [
        "new_epoch",
        "bootstrap",
        "head",
        "stop",
        "force",
        "activate",
        "target_writer",
    ] {
        let mut value = json_codec::json!({
            "service_user":"zxs", "external_writers_stopped":true, "plan":{},
            "confirmation":"prepare-chain-activation-plan-without-consuming-stop",
            "acknowledge_plan_without_stop_consumption":true
        });
        value[field] = true.into();
        assert!(
            json_codec::from_value::<crate::offline::disk_removal::chain_target::ActivationPlanInput>(value)
                .is_err()
        );
    }
}

#[test]
fn chain_io_decision_input_rejects_caller_supplied_vote_epoch_or_active_writer() {
    for field in [
        "new_epoch",
        "decision",
        "receipt",
        "target_root",
        "bootstrap",
        "force",
        "consume_stop",
        "target_writer",
    ] {
        // Put the injected field first and require that exact parser error;
        // an intentionally incomplete nested plan must not make this pass.
        let value = format!(
            r#"{{"{field}":true,"service_user":"zxs","external_writers_stopped":true,"plan":{{}},"confirmation":"decide-chain-io-epoch-without-activation","acknowledge_irreversible_decision_without_activation":true}}"#
        );
        let error = match json_codec::from_str::<
            crate::offline::disk_removal::chain_target::IoDecisionInput,
        >(&value)
        {
            Ok(_) => panic!("injected field was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unknown field"));
        assert!(error.to_string().contains(field));
    }
}

#[test]
fn chain_generation_input_rejects_uploaded_log_or_active_authority_overrides() {
    for field in [
        "journal",
        "native_objects",
        "generation",
        "target_root",
        "bootstrap",
        "force",
        "consume_stop",
        "writer",
    ] {
        let value = format!(
            r#"{{"{field}":true,"service_user":"zxs","external_writers_stopped":true,"plan":{{}},"confirmation":"prepare-chain-generation-log-without-active-writer","acknowledge_generation_log_without_active_writer":true}}"#
        );
        let error = match json_codec::from_str::<
            crate::offline::disk_removal::chain_target::GenerationLogInput,
        >(&value)
        {
            Ok(_) => panic!("injected authority field was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unknown field"));
        assert!(error.to_string().contains(field));
    }
}

#[test]
fn chain_selector_input_rejects_uploaded_authority_and_stop_overrides() {
    for field in [
        "selector",
        "authority",
        "generation",
        "io_epoch",
        "target_root",
        "consume_stop",
        "writer",
        "force",
    ] {
        let value = format!(
            r#"{{"{field}":true,"service_user":"zxs","external_writers_stopped":true,"plan":{{}},"confirmation":"select-chain-generation-without-active-writer","acknowledge_selector_without_active_writer":true}}"#
        );
        let error = match json_codec::from_str::<
            crate::offline::disk_removal::chain_target::GenerationSelectionInput,
        >(&value)
        {
            Ok(_) => panic!("injected selector field was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unknown field"));
        assert!(error.to_string().contains(field));
    }
}

#[tokio::test]
async fn foreign_chain_routes_keep_request_gates_without_accessing_system_or_disks() {
    let app = Dispatcher;
    let preview = json!({"service_user":"zxs","operation_id":"op_11111111111111111111111111111111",
        "source_root":"/not-opened","source_slot_id":"secondary","source_upstream_slot_id":"primary",
        "source_metadata_member":true,"lower":[],"external_writers_stopped":false});
    let original = json!({"service_user":"zxs","operation_id":"op_11111111111111111111111111111111",
        "request_digest":"a".repeat(64),"external_writers_stopped":false});
    for (path, input) in [
        ("/api/disks/foreign-chain-preview", preview),
        ("/api/disks/foreign-chain-original", original),
    ] {
        let response = app.clone().oneshot(post(path, input)).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = json_body(response).await;
        assert_eq!(body["error"]["code"], "foreign_chain_material");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("stop ZXS")
        );
    }
    let response = app
        .oneshot(post(
            "/api/disks/foreign-chain-prepare",
            json!({"confirmation":"admit-foreign-now"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn terminal_chain_export_requires_explicit_stopped_writers() {
    let response = Dispatcher
        .oneshot(post(
            "/api/disks/terminal-chain-export",
            json!({"service_user":"zxs","external_writers_stopped":false}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = Dispatcher.oneshot(post("/api/disks/terminal-chain-export", json!({"service_user":"zxs","external_writers_stopped":true,"foreign_chain":"untrusted","activate":true}))).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn chain_publication_rejects_unselected_recipe_and_unsupported_activation_controls() {
    let path = "/api/disks/terminal-chain-first-publish";
    for field in [
        "force",
        "activate",
        "root_path",
        "foreign_chain",
        "publication",
        "seal",
        "journal",
        "candidate",
    ] {
        let mut input = json!({
            "service_user":"zxs", "external_writers_stopped":true,
            "confirmation":"publish-chain-first-slot-without-activation",
            "acknowledge_publication_without_activation":true, "plan":{},
        });
        input[field] = "untrusted".into();
        assert_eq!(
            Dispatcher
                .oneshot(post(path, input))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}

#[test]
fn credential_pipe_diagnostics_do_not_echo_malformed_secret_values() {
    for operation in [
        Operation::AdministratorReset,
        Operation::FirstAdministratorCreate,
    ] {
        let error = dispatch(
            operation,
            br#"{"password":"do-not-echo-secret","new_password":"do-not-echo-secret"}"#,
        )
        .unwrap_err();
        assert_eq!(error.kind, FailureKind::InvalidRequest);
        assert!(!error.message.contains("do-not-echo-secret"));
    }
}
