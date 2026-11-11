use super::*;
use name_policy::{Name, PortablePath, SlotId};
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
pub(super) struct NamedHistory<'a> {
    pub(super) history: &'a storage_contract::storage_foreign::ForeignHistorySummary,
    pub(super) name: Name,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReconstructionTarget<'a> {
    slot: SlotId,
    parent: Option<PortablePath>,
    names: Vec<NamedHistory<'a>>,
}

/// Local design intents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Intent<'a> {
    ReconstructExistingPrimary(ReconstructionTarget<'a>),
    AdmitNewPrimary {
        copy_target: Option<CandidateSelection>,
        slot: SlotId,
        names: Vec<NamedHistory<'a>>,
    },
    AdmitSecondary {
        copy_target: Option<CandidateSelection>,
        slot: SlotId,
        primary: PrimaryChoice,
        names: Vec<NamedHistory<'a>>,
    },
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DraftError {
    Slot(name_policy::Error),
    Parent(name_policy::Error),
    CompleteMapping,
    Name(name_policy::Error),
    Collision,
    SourceTargetAlias,
    SourceTier,
    ExplicitPrimary,
    PrimaryChanged,
    PreviewUnavailable,
}
impl Draft {
    pub(super) fn intent<'a>(
        &self,
        source: &'a ForeignDiskPreview,
        primaries: &[PrimaryChoice],
    ) -> Result<Intent<'a>, DraftError> {
        let slot = SlotId::parse(&self.slot).map_err(DraftError::Slot)?;
        let groups: BTreeMap<_, _> = source
            .histories
            .iter()
            .map(|history| (&history.group_id, history))
            .collect();
        if groups.is_empty()
            || self.names.keys().collect::<BTreeSet<_>>()
                != groups.keys().copied().collect::<BTreeSet<_>>()
        {
            return Err(DraftError::CompleteMapping);
        }
        let mut keys = BTreeSet::new();
        let mut names = Vec::with_capacity(self.names.len());
        for (group, name) in &self.names {
            let name = Name::parse(name).map_err(DraftError::Name)?;
            if !keys.insert(name.key()) {
                return Err(DraftError::Collision);
            }
            names.push(NamedHistory {
                history: groups[&group],
                name,
            });
        }
        if self.mode == Mode::ReconstructExistingPrimary {
            let parent = (!self.parent.is_empty())
                .then(|| PortablePath::parse(&self.parent))
                .transpose()
                .map_err(DraftError::Parent)?;
            return Ok(Intent::ReconstructExistingPrimary(ReconstructionTarget {
                slot,
                parent,
                names,
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
                slot,
                names,
            });
        }
        if source.source_level != 3 {
            return Err(DraftError::SourceTier);
        }
        let primary = self.primary.as_ref().ok_or(DraftError::ExplicitPrimary)?;
        if !primaries.contains(primary)
            || !super::super::identity_valid(&primary.identity)
            || primary.identity.slot_id.eq_ignore_ascii_case(slot.as_str())
        {
            return Err(DraftError::PrimaryChanged);
        }
        Ok(Intent::AdmitSecondary {
            copy_target: self.empty_target.clone(),
            slot,
            primary: primary.clone(),
            names,
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
            target: ForeignPlanTarget {
                slot_id: target.slot.to_string(),
                parent: target.parent.map(|path| path.as_str().to_owned()),
                history_names: target.names
                    .into_iter()
                    .map(|named| (named.history.group_id.clone(), named.name.to_string()))
                    .collect(),
            },
        })
    }
}
