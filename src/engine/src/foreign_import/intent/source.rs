//! Current-format source commitments. These cannot be deserialized into a writer.
use super::super::migration::{InspectionBudget, Inventory, PhysicalBinding};
use super::*;

/// A complete source observation held through review. Capture performs bounded
/// disk I/O; callers must keep it outside the engine lock and recheck admission.
pub struct ForeignImportIntentSource {
    pub(super) preview: ForeignImportPreview,
    pub(super) record: SourceRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct SourceRecord {
    registration: DiskRegistrationIdentity,
    level: u32,
    path: PathBuf,
    physical: PhysicalBinding,
    seal_digest: String,
    review_digest: String,
    candidate_hash: String,
    publication_hash: String,
    sequence: u64,
    #[serialization(deserialize_with = "required_nullable")]
    authority_hash: Option<String>,
    unpublished_tail: bool,
    histories: Vec<HistoryRecord>,
    inventory: Inventory,
    budget: InspectionBudget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct HistoryRecord {
    group_id: String,
    origin: DiskRegistrationIdentity,
    history_tree_id: String,
    manifest_hash: String,
    availability: BTreeMap<String, Availability>,
    origin_publications: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum Availability {
    VerifiedLocal,
    TemporarilyUnavailable,
    RecordedPurged,
    RecordedMigrated,
}
impl From<ForeignVersionAvailability> for Availability {
    fn from(value: ForeignVersionAvailability) -> Self {
        match value {
            ForeignVersionAvailability::VerifiedLocal => Self::VerifiedLocal,
            ForeignVersionAvailability::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ForeignVersionAvailability::RecordedPurged => Self::RecordedPurged,
            ForeignVersionAvailability::RecordedMigrated => Self::RecordedMigrated,
        }
    }
}

impl ForeignImportIntentSource {
    /// Re-inspect the full metadata/body closure and freeze every raw source
    /// file. Missing bodies remain in the complete history as unavailable.
    pub fn capture(preview: ForeignImportPreview) -> Result<Self> {
        if preview.histories.is_empty() {
            return Err(invalid(
                "foreign import requires at least one complete history",
            ));
        }
        let before = ForeignImportPreview::inspect(
            preview.root.directory(preview.root.path())?,
            preview.root.path().into(),
            preview.limits,
        )?;
        if before.digest != preview.digest {
            return Err(stale(
                "foreign source changed after its reviewed observation",
            ));
        }
        let budget = InspectionBudget::from_limits(preview.limits);
        let inventory = Inventory::capture(&preview.root, &budget)?;
        let after = ForeignImportPreview::inspect(
            preview.root.directory(preview.root.path())?,
            preview.root.path().into(),
            preview.limits,
        )?;
        if after.digest != preview.digest {
            return Err(stale(
                "foreign source changed while capturing its complete inventory",
            ));
        }
        let histories = preview
            .histories
            .iter()
            .map(|history| HistoryRecord {
                group_id: history.group_id.clone(),
                origin: DiskRegistrationIdentity {
                    pool_id: history.origin.pool_id.clone(),
                    disk_id: history.origin.disk_id.clone(),
                    slot_id: history.origin.slot_id.clone(),
                    binding_generation: history.origin.binding_generation,
                },
                history_tree_id: history.manifest.history.history_tree_id.clone(),
                manifest_hash: history.manifest_hash.clone(),
                availability: history
                    .availability
                    .iter()
                    .map(|(id, state)| (id.clone(), (*state).into()))
                    .collect(),
                origin_publications: history.origin_publications.clone(),
            })
            .collect();
        let record = SourceRecord {
            registration: preview.source.registration.clone(),
            level: preview.source_level,
            path: preview.root.path().into(),
            physical: PhysicalBinding::of(&preview.root)?,
            seal_digest: preview.source.seal_digest.to_string(),
            review_digest: preview.digest.to_string(),
            candidate_hash: preview.checkpoint.candidate_hash.clone(),
            publication_hash: preview.checkpoint.publication_hash.clone(),
            sequence: preview.checkpoint.sequence,
            authority_hash: preview.checkpoint.authority_hash.clone(),
            unpublished_tail: preview.checkpoint.unpublished_tail,
            histories,
            inventory,
            budget,
        };
        record.validate()?;
        Ok(Self { preview, record })
    }

    pub fn preview(&self) -> &ForeignImportPreview {
        &self.preview
    }
}

impl SourceRecord {
    pub(super) fn validate(&self) -> Result<()> {
        identity(&self.registration)?;
        if !(1..=3).contains(&self.level) || !self.path.is_absolute() || self.histories.is_empty() {
            return Err(invalid(
                "invalid complete foreign source identity or history set",
            ));
        }
        self.budget.limits()?;
        self.inventory.validate(&self.budget)?;
        for value in [
            &self.seal_digest,
            &self.review_digest,
            &self.candidate_hash,
            &self.publication_hash,
        ]
        .into_iter()
        .chain(self.authority_hash.as_ref())
        {
            digest_value(value)?;
        }
        let mut groups = std::collections::BTreeSet::new();
        for history in &self.histories {
            identity(&history.origin)?;
            validate_id(&history.history_tree_id, "history")?;
            digest_value(&history.manifest_hash)?;
            let expected = object_hash(
                "foreign-history-group",
                &json_codec::json!({
                    "pool":history.origin.pool_id,"disk":history.origin.disk_id,
                    "binding":history.origin.binding_generation.to_string(),"history":history.history_tree_id,
                }),
            )?;
            if history.group_id != expected
                || !groups.insert(&history.group_id)
                || history
                    .availability
                    .keys()
                    .ne(history.origin_publications.keys())
            {
                return Err(invalid(
                    "foreign source history groups or provenance differ",
                ));
            }
            for (version, publication) in &history.origin_publications {
                validate_id(version, "ver")?;
                digest_value(publication)?;
            }
        }
        Ok(())
    }

    pub(super) fn groups(&self) -> impl Iterator<Item = (&str, &str)> {
        self.histories
            .iter()
            .map(|history| (history.group_id.as_str(), history.history_tree_id.as_str()))
    }

    pub(super) fn unavailable_count(&self) -> usize {
        self.histories
            .iter()
            .flat_map(|history| history.availability.values())
            .filter(|state| **state == Availability::TemporarilyUnavailable)
            .count()
    }

    pub(super) fn review_digest(&self) -> &str {
        &self.review_digest
    }
}
