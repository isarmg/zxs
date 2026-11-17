//! The actual non-Version material admitted by the ordinary publication path.
//! A descriptive manifest or request cannot stand in for its held preparation.
use super::DraftPublicationInput;
use super::unborn::save::UnbornPublicationInput;
use crate::engine::PlanningContext;
use crate::{OperationResult, Result, State};
use storage_contract::FileId;
use storage_contract::file_draft::{DraftPayload, UnbornBody};

#[derive(Clone, Copy, PartialEq)]
enum Material {
    Born(FileId),
    Incremental(FileId),
    Committed(FileId),
    Unborn(FileId),
}

pub(in crate::engine) enum WorkingPublicationInput {
    Born(DraftPublicationInput),
    Incremental(super::incremental::IncrementalPublicationInput),
    Committed(super::incremental::CommittedDraftInput),
    Unborn(UnbornPublicationInput),
}

impl WorkingPublicationInput {
    fn material(&self) -> Material {
        match self {
            Self::Born(input) => Material::Born(input.file()),
            Self::Incremental(input) => Material::Incremental(input.file()),
            Self::Committed(input) => Material::Committed(input.file()),
            Self::Unborn(input) => Material::Unborn(input.file()),
        }
    }

    /// All introduced saved material needs its actual hot producer. Empty Birth
    /// is separately issued by the complete birth proof and has no saved bytes.
    pub(in crate::engine) fn require_input(
        input: Option<&Self>,
        context: &PlanningContext<'_>,
        next: &State,
        result: &OperationResult,
    ) -> Result<()> {
        let mut introduced = None;
        let mut inspect = |file: &FileId| -> Result<()> {
            let previous = context.state.file_containers.get(file);
            let current = next.file_containers.get(file);
            let body = current.and_then(|record| record.working.draft());
            let old_body = previous.and_then(|record| record.working.draft());
            let material = if body.map(|(_, payload)| payload)
                != old_body.map(|(_, payload)| payload) && body.is_some()
            {
                Some(if current.and_then(|record| record.working.incremental()).is_some() {
                    Material::Incremental(*file)
                } else {
                    Material::Born(*file)
                })
            } else if result.kind == "file.draft.commit"
                && previous.and_then(|record| record.working.incremental()).is_some()
                && body.is_none()
            {
                Some(Material::Committed(*file))
            } else {
                let body = current.and_then(|record| record.working.unborn())
                    .map(|working| working.body());
                let old_body = previous.and_then(|record| record.working.unborn())
                    .map(|working| working.body());
                if matches!(body, Some(UnbornBody::Saved { .. })) && body != old_body {
                    Some(Material::Unborn(*file))
                } else {
                    None
                }
            };
            if let Some(material) = material {
                if introduced.replace(material).is_some() {
                    return Err(
                        crate::engine::fresh_layout::FreshLayoutError::ContainerScope.into(),
                    );
                }
            }
            Ok(())
        };
        match next.file_containers.changes_since(&context.state.file_containers) {
            storage_contract::MapChanges::Keys(keys) => {
                for file in keys.iter() { inspect(file)?; }
            }
            storage_contract::MapChanges::All => {
                for file in next.file_containers.keys() { inspect(file)?; }
            }
        }
        if introduced != input.map(Self::material) {
            return Err(crate::engine::fresh_layout::FreshLayoutError::ContainerScope.into());
        }
        if let Some(input) = input {
            input.require_context(context)?;
            input.require_next(next, result)?;
        }
        Ok(())
    }

    pub(in crate::engine) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        match self {
            Self::Born(input) => input.require_context(context),
            Self::Incremental(input) => input.require_context(context),
            Self::Committed(input) => input.require_context(context),
            Self::Unborn(input) => input.require_context(context),
        }
    }

    fn require_next(&self, next: &State, result: &OperationResult) -> Result<()> {
        match self {
            Self::Born(input) => input.require_next(next, result),
            Self::Incremental(input) => input.require_next(next, result),
            Self::Committed(input) => input.require_next(next, result),
            Self::Unborn(input) => input.require_next(next, result),
        }
    }

    pub(in crate::engine) fn commit_read(&self)
        -> Option<&std::sync::Arc<crate::container_storage::ReadDraftDigest>>
    {
        match self {
            Self::Committed(input) => Some(input.read()),
            Self::Born(_) | Self::Unborn(_) | Self::Incremental(_) => None,
        }
    }

    pub(in crate::engine) fn draft_mut(&mut self) -> Option<&mut DraftPublicationInput> {
        match self {
            Self::Born(input) => Some(input),
            Self::Unborn(_) | Self::Incremental(_) | Self::Committed(_) => None,
        }
    }

    pub(in crate::engine) fn new_draft(&self) -> Option<(FileId, DraftPayload)> {
        match self {
            Self::Born(input) => Some((input.file(), input.payload())),
            // The extent writer already reserved and synced its actual bytes.
            // Publication still charges its exact native metadata as usual.
            Self::Unborn(_) | Self::Incremental(_) | Self::Committed(_) => None,
        }
    }
}
