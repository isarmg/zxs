use super::http::Failure;
use data_serialization::Deserialize;
use http_routing::http::StatusCode;
use storage_engine::disk_authority::{Digest, DiskSelection, Error, IndependentDisk};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Check {
    pub selection: DiskSelection,
    pub writers_stopped: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Export {
    pub selection: DiskSelection,
    pub writers_stopped: bool,
    pub commit: Digest,
}

pub(super) fn admit(selection: &DiskSelection, writers_stopped: bool) -> Result<(), Failure> {
    if !writers_stopped {
        return Err(Failure::message(
            StatusCode::CONFLICT,
            "writers_not_stopped",
            "Stop every source writer and explicitly acknowledge this before checking or exporting. This acknowledgment is not a service lock.",
        ));
    }
    if !selection.path.is_absolute() {
        return Err(Failure::message(
            StatusCode::BAD_REQUEST,
            "disk_path",
            "Select an absolute disk path and its exact original identity.",
        ));
    }
    Ok(())
}

pub(super) struct VerifiedExport {
    pub bytes: Vec<u8>,
    pub commit: Digest,
    pub body: Digest,
    pub checkpoint: Option<Digest>,
}

pub(super) fn export(input: Export) -> Result<VerifiedExport, Failure> {
    let disk = IndependentDisk::open(&input.selection).map_err(disk_error)?;
    let mut snapshot = disk.content_snapshot().map_err(disk_error)?;
    let body = snapshot
        .versions()
        .find(|copy| copy.version().commit() == input.commit)
        .map(|copy| copy.version().body())
        .ok_or_else(|| disk_error(Error::Missing(input.commit)))?;
    let bytes = snapshot.read_content(input.commit).map_err(disk_error)?;
    snapshot.recheck().map_err(disk_error)?;
    Ok(VerifiedExport {
        bytes,
        commit: input.commit,
        body,
        checkpoint: snapshot.checkpoint(),
    })
}

pub(super) fn disk_error(error: Error) -> Failure {
    let (status, code) = match &error {
        Error::Identity | Error::StaleBase => (StatusCode::CONFLICT, "disk_identity_or_checkpoint"),
        Error::Missing(_) => (StatusCode::CONFLICT, "published_metadata_missing"),
        Error::ContentUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "content_unavailable"),
        Error::Budget => (StatusCode::UNPROCESSABLE_ENTITY, "disk_budget"),
        Error::Corrupt(_) | Error::RequiredBodyMissing(_) => {
            (StatusCode::UNPROCESSABLE_ENTITY, "disk_integrity")
        }
        Error::Invalid(_) | Error::Json(_) => (StatusCode::UNPROCESSABLE_ENTITY, "disk_protocol"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "disk_access"),
    };
    Failure::new(status, code, error)
}
