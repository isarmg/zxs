use super::*;

const ALIASES: &[(ErrorCode, ErrorCode)] = &[
    (ErrorCode::PermissionDenied, ErrorCode::Forbidden),
    (ErrorCode::Unauthenticated, ErrorCode::Unauthorized),
    (ErrorCode::FileVersionConflict, ErrorCode::FileConflict),
    (ErrorCode::BaseHeadMismatch, ErrorCode::HeadConflict),
    (ErrorCode::FileBusy, ErrorCode::FileWriteBusy),
    (ErrorCode::FencingTokenInvalid, ErrorCode::Fenced),
    (ErrorCode::FencingTokenExpired, ErrorCode::LeaseExpired),
    (ErrorCode::ResourceLimit, ErrorCode::ResourceLimitExceeded),
    (ErrorCode::RouteIncomplete, ErrorCode::BackupRouteIncomplete),
    (ErrorCode::InDoubt, ErrorCode::CommitInDoubt),
    (ErrorCode::GenerationMismatch, ErrorCode::ConfigConflict),
    (
        ErrorCode::StaleUploadGeneration,
        ErrorCode::UploadGenerationConflict,
    ),
];

const PUBLIC_STATUS: &[(ErrorCode, ::http::StatusCode)] = &[
    (ErrorCode::Unauthorized, ::http::StatusCode::UNAUTHORIZED),
    (ErrorCode::Forbidden, ::http::StatusCode::FORBIDDEN),
    (ErrorCode::ContentPurged, ::http::StatusCode::GONE),
    (
        ErrorCode::HttpPreconditionFailed,
        ::http::StatusCode::PRECONDITION_FAILED,
    ),
    (ErrorCode::NoSpace, ::http::StatusCode::INSUFFICIENT_STORAGE),
    (
        ErrorCode::ResourceLimitExceeded,
        ::http::StatusCode::TOO_MANY_REQUESTS,
    ),
    (
        ErrorCode::QuotaExceeded,
        ::http::StatusCode::TOO_MANY_REQUESTS,
    ),
    (
        ErrorCode::ContentUnavailable,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::MetadataUnavailable,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::RecoveryRequired,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::ReplicaTargetUnavailable,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::RouteTargetUnavailable,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::CommitInDoubt,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::BackupRouteIncomplete,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::CommitPreparationBusy,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::CommitPreparationStale,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::PreparationStale, ::http::StatusCode::CONFLICT),
    (ErrorCode::HeadConflict, ::http::StatusCode::CONFLICT),
    (ErrorCode::HistoryChanged, ::http::StatusCode::CONFLICT),
    (ErrorCode::ListingChanged, ::http::StatusCode::CONFLICT),
    (ErrorCode::BranchNotActive, ::http::StatusCode::CONFLICT),
    (ErrorCode::Fenced, ::http::StatusCode::CONFLICT),
    (ErrorCode::LeaseExpired, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::UploadGenerationConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::FileConflict, ::http::StatusCode::CONFLICT),
    (ErrorCode::FileBranchConflict, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::DirectoryBranchConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::DirectoryAliasConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::BatchMappingConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::BatchScopeConflict, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::IdempotencyKeyReused,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::SplitPlanStale, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::PurgeBlockedByReference,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::ContentPinned, ::http::StatusCode::CONFLICT),
    (ErrorCode::ConfigConflict, ::http::StatusCode::CONFLICT),
    (ErrorCode::BranchWriteBusy, ::http::StatusCode::CONFLICT),
    (ErrorCode::FileWriteBusy, ::http::StatusCode::CONFLICT),
    (ErrorCode::NamespaceConflict, ::http::StatusCode::CONFLICT),
    (ErrorCode::NameCollision, ::http::StatusCode::CONFLICT),
    (ErrorCode::WorkspaceClosed, ::http::StatusCode::CONFLICT),
    (ErrorCode::PlanStale, ::http::StatusCode::CONFLICT),
    (ErrorCode::PlanClosed, ::http::StatusCode::CONFLICT),
    (ErrorCode::TreeMoved, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::RestoreBackupConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::DirectoryBranchIncomplete,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::DirectoryScopeRequired,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::DirectoryHistorySplitUnsupported,
        ::http::StatusCode::BAD_REQUEST,
    ),
    (
        ErrorCode::FileNodeHistorySplitUnsupported,
        ::http::StatusCode::BAD_REQUEST,
    ),
    (
        ErrorCode::IoError,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::DurabilityUnknown,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::StateInvalid,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::InternalError,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::ObjectCorrupt,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::LogCorrupt,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::PublicationInvalid,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::OperationUnsupported,
        ::http::StatusCode::NOT_IMPLEMENTED,
    ),
    (
        ErrorCode::PlanUnsupported,
        ::http::StatusCode::NOT_IMPLEMENTED,
    ),
];

#[test]
fn durability_unknown_is_retryable_unknown_and_diagnostics_are_private() {
    let mapped = super::super::wire::error(storage_engine::Error::new(
        storage_contract::ErrorCode::DurabilityUnknown,
        "private /srv/zxs/disk/config.json directory sync failure after publication",
    ));
    assert_eq!(mapped.status, 500);
    assert_eq!(mapped.code, storage_contract::ErrorCode::DurabilityUnknown);
    assert!(mapped.retryable);
    assert!(storage_contract::client_protocol::mutation_outcome_unknown(
        mapped.status.as_u16()
    ));
    assert!(!mapped.message.contains("private"));
    assert!(!mapped.message.contains("/srv/zxs"));
}

#[test]
fn registrations_are_unique_and_alias_targets_have_explicit_status() {
    let mut internal = std::collections::BTreeSet::new();
    let mut public = std::collections::BTreeSet::new();
    for &(code, http) in PUBLIC_STATUS {
        assert!(public.insert(code), "duplicate public code {code}");
        assert!((400..=599).contains(&http.as_u16()));
    }
    for &(source, target) in ALIASES {
        assert!(internal.insert(source), "duplicate alias {source}");
        assert!(public.contains(&target), "unregistered target {target}");
        assert_eq!(public_code(source), target);
    }
}

#[test]
fn wire_mapping_preserves_status_retry_and_diagnostic_policy() {
    for &(code, expected) in PUBLIC_STATUS {
        let mapped =
            super::super::wire::error(storage_engine::Error::new(code, "private diagnostic"));
        assert_eq!(mapped.code, code);
        assert_eq!(mapped.status, expected, "{code}");
        assert_eq!(
            mapped.retryable,
            (expected.is_server_error() && expected != ::http::StatusCode::NOT_IMPLEMENTED)
                || expected == ::http::StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            mapped.message == "private diagnostic",
            expected != ::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }
    for &(internal, public) in ALIASES {
        let mapped = super::super::wire::error(storage_engine::Error::new(internal, "detail"));
        assert_eq!(mapped.code, public);
        assert_eq!(mapped.status, status(public));
    }
    for (code, expected) in [
        (storage_contract::ErrorCode::NotFound, 404),
        (storage_contract::ErrorCode::WorkspaceNotFound, 404),
        (storage_contract::ErrorCode::InjectedFailure, 400),
    ] {
        assert_eq!(status(code), expected);
        assert_eq!(public_code(code), code);
    }
    let error = storage_engine::Error::new(storage_contract::ErrorCode::InjectedFailure, "detail");
    let error = error.retryable();
    assert!(super::super::wire::error(error).retryable);
}

#[test]
fn engine_mapping_keeps_io_source_while_redacting_private_diagnostics() {
    use std::error::Error as _;
    let error = super::super::wire::error(storage_engine::Error::from(std::io::Error::other(
        "private /srv/zxs/objects I/O diagnostic",
    )));
    assert_eq!(error.status, 500);
    assert_eq!(error.code, storage_contract::ErrorCode::IoError);
    assert!(error.retryable);
    let mut source = error.source();
    let mut found = false;
    while let Some(cause) = source {
        if cause.is::<std::io::Error>() {
            found = true;
            assert!(cause.to_string().contains("/srv/zxs/objects"));
        }
        source = cause.source();
    }
    assert!(found, "concrete I/O cause missing from error chain");
    assert!(!json_codec::to_string(&error).unwrap().contains("/srv/zxs"));
}

#[test]
fn all_error_identities_keep_their_original_mapping() {
    // The expectations are retained independently from the production declaration.
    use ::http::StatusCode;
    let not_found = [
        ErrorCode::BranchNotFound,
        ErrorCode::CommitNotFound,
        ErrorCode::DirectoryBranchNotFound,
        ErrorCode::DirectoryNotFound,
        ErrorCode::DiskNotFound,
        ErrorCode::EntryNotFound,
        ErrorCode::FileBranchNotFound,
        ErrorCode::FileNodeNotFound,
        ErrorCode::FileNotFound,
        ErrorCode::HistoryNodeNotFound,
        ErrorCode::HistoryNotFound,
        ErrorCode::ItemNotFound,
        ErrorCode::NotFound,
        ErrorCode::OperationNotFound,
        ErrorCode::PlacementNotFound,
        ErrorCode::PlanNotFound,
        ErrorCode::RecoveryPlanNotFound,
        ErrorCode::RepositoryNotFound,
        ErrorCode::SlotNotFound,
        ErrorCode::VersionNotFound,
        ErrorCode::WorkspaceNotFound,
    ];
    for &code in ErrorCode::ALL {
        let public = ALIASES
            .iter()
            .find_map(|&(internal, public)| (internal == code).then_some(public))
            .unwrap_or(code);
        assert_eq!(public_code(code), public, "public identity for {code}");
        let raw_status = PUBLIC_STATUS
            .iter()
            .find_map(|&(registered, status)| (registered == code).then_some(status))
            .unwrap_or_else(|| {
                if not_found.contains(&code) {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::BAD_REQUEST
                }
            });
        assert_eq!(status(code), raw_status, "raw status for {code}");
        let mapped = super::super::wire::error(storage_engine::Error::new(code, "detail"));
        assert_eq!(mapped.code, public, "wire identity for {code}");
        assert_eq!(mapped.status, status(public), "wire status for {code}");
    }
}
