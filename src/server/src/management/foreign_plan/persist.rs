//! An immutable audit copy of the reviewed wire message, never engine authority.
use super::*;
use data_serialization::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "foreign-reconstruction-plan")]
    Review,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct StoredPlan {
    format: Format,
    principal: String,
    pub(super) review: ForeignPlanPreview,
}
impl StoredPlan {
    pub(super) fn new(principal: String, review: ForeignPlanPreview) -> Self {
        Self {
            format: Format::Review,
            principal,
            review,
        }
    }
    pub(super) fn status(&self, validation: ForeignPlanValidation) -> ForeignPlanStatus {
        ForeignPlanStatus {
            preview: self.review.clone(),
            validation,
        }
    }
    pub(super) fn require_digest(&self, digest: &str) -> Result<(), ApiError> {
        if self.review.review_digest == digest {
            Ok(())
        } else {
            Err(ApiError::conflict(
                ErrorCode::PlanStale,
                "foreign plan review digest differs",
            ))
        }
    }
}

pub(super) fn open(root: &Path) -> Result<Option<PrivateDirectory>, ApiError> {
    match PrivateDirectory::open_existing(root.join(DIRECTORY)) {
        Ok(directory) => Ok(Some(directory)),
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(source) => Err(storage_addition::storage_error(source)),
    }
}

pub(super) fn read(
    directory: &PrivateDirectory,
    operation: OperationId,
) -> Result<Option<StoredPlan>, ApiError> {
    let bytes = match directory.read_bounded(&storage_addition::name(operation), MAX_RECORD_BYTES) {
        Ok(bytes) => bytes,
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(source) => return Err(storage_addition::storage_error(source)),
    };
    let value = storage_contract::strict_json(&bytes).map_err(ApiError::invalid_source)?;
    let record: StoredPlan = json_codec::from_value(value).map_err(ApiError::invalid_source)?;
    validate_review(&record.review)?;
    if record.review.request.operation_id != operation {
        return Err(ApiError::conflict(
            ErrorCode::ObjectCorrupt,
            "foreign plan filename and identity differ",
        ));
    }
    Ok(Some(record))
}

pub(super) fn owned(record: &StoredPlan, identity: &Identity) -> Result<(), ApiError> {
    if record.principal == identity.principal {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

pub(super) fn store(directory: &PrivateDirectory, record: &StoredPlan) -> Result<(), ApiError> {
    let bytes = crate::history_window_encode::encode_bounded(record, MAX_RECORD_BYTES)?;
    AtomicFile::create(
        directory,
        &storage_addition::name(record.review.request.operation_id),
        &bytes,
    )
    .map_err(storage_addition::storage_error)
}
