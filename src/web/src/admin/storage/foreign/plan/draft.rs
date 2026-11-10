use super::*;
use storage_contract::storage_directories::CandidateSelection;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Mode {
    #[default]
    ReconstructExistingPrimary,
    AdmitNewPrimary,
    AdmitSecondary,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PrimaryChoice {
    pub(super) identity: ForeignDiskIdentity,
    pub(super) sequence: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Draft {
    pub(super) mode: Mode,
    pub(super) slot: String,
    pub(super) parent: String,
    pub(super) names: BTreeMap<String, String>,
    pub(super) empty_target: Option<CandidateSelection>,
    pub(super) primary: Option<PrimaryChoice>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Intent {
    ReconstructExistingPrimary(ForeignPlanTarget),
    AdmitNewPrimary {
        copy_target: Option<CandidateSelection>,
        slot: String,
        names: BTreeMap<String, String>,
    },
    AdmitSecondary {
        copy_target: Option<CandidateSelection>,
        slot: String,
        primary: PrimaryChoice,
        names: BTreeMap<String, String>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DraftError {
    Slot,
    Parent,
    CompleteMapping,
    Name,
    Collision,
    SourceTargetAlias,
    SourceTier,
    ExplicitPrimary,
    PrimaryChanged,
    PreviewUnavailable,
}
impl Draft {
    pub(super) fn intent(
        &self,
        source: &ForeignDiskPreview,
        primaries: &[PrimaryChoice],
    ) -> Result<Intent, DraftError> {
        name_policy::validate_slot(&self.slot).map_err(|_| DraftError::Slot)?;
        let groups: BTreeSet<_> = source
            .histories
            .iter()
            .map(|history| &history.group_id)
            .collect();
        if groups.is_empty() || self.names.keys().collect::<BTreeSet<_>>() != groups {
            return Err(DraftError::CompleteMapping);
        }
        let mut keys = BTreeSet::new();
        for name in self.names.values() {
            name_policy::validate_name(name).map_err(|_| DraftError::Name)?;
            if !keys.insert(name_policy::lookup_key(name)) {
                return Err(DraftError::Collision);
            }
        }
        if self.mode == Mode::ReconstructExistingPrimary {
            let parent = (!self.parent.is_empty()).then(|| self.parent.clone());
            if parent
                .as_ref()
                .is_some_and(|path| name_policy::validate_path(path).is_err())
            {
                return Err(DraftError::Parent);
            }
            return Ok(Intent::ReconstructExistingPrimary(ForeignPlanTarget {
                slot_id: self.slot.clone(),
                parent,
                history_names: self.names.clone(),
            }));
        }
        if self.empty_target.as_ref().is_some_and(|target| {
            target.root_path == source.source.path
                || (target.root_identity.device == source.source.identity.device
                    && target.root_identity.inode == source.source.identity.inode)
        }) {
            return Err(DraftError::SourceTargetAlias);
        }
        if self.mode == Mode::AdmitNewPrimary {
            return Ok(Intent::AdmitNewPrimary {
                copy_target: self.empty_target.clone(),
                slot: self.slot.clone(),
                names: self.names.clone(),
            });
        }
        if source.source_level != 3 {
            return Err(DraftError::SourceTier);
        }
        let primary = self.primary.as_ref().ok_or(DraftError::ExplicitPrimary)?;
        if !primaries.contains(primary)
            || !super::super::identity_valid(&primary.identity)
            || primary.identity.slot_id.eq_ignore_ascii_case(&self.slot)
        {
            return Err(DraftError::PrimaryChanged);
        }
        Ok(Intent::AdmitSecondary {
            copy_target: self.empty_target.clone(),
            slot: self.slot.clone(),
            primary: primary.clone(),
            names: self.names.clone(),
        })
    }
    pub(super) fn request(
        &self,
        source: &ForeignDiskPreview,
        primaries: &[PrimaryChoice],
        operation_id: OperationId,
    ) -> Result<ForeignPlanRequest, DraftError> {
        let Intent::ReconstructExistingPrimary(target) = self.intent(source, primaries)? else {
            return Err(DraftError::PreviewUnavailable);
        };
        Ok(ForeignPlanRequest {
            operation_id,
            source: source.source.clone(),
            source_review_digest: source.review_digest.clone(),
            target,
        })
    }
}
