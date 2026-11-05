//! Per-primary directories point into the unmodified source proof graph. These
//! are import descriptors, never replacement directory-commit proofs or bodies.
use super::*;

#[derive(Serialize)]
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

#[derive(Serialize)]
struct HistoryDescriptor<'a> {
    history_tree_id: &'a str,
    manifest_hash: &'a str,
    availability: BTreeMap<&'a str, Availability>,
}

pub(super) struct Projection {
    files: BTreeMap<PathBuf, Vec<u8>>,
    inventory: Inventory,
}
impl Projection {
    pub(super) fn from_replica(plan: &PlanRecord, replica: &ForeignImportPreview) -> Result<Self> {
        let mut files = BTreeMap::new();
        let mut directories = BTreeSet::new();
        let histories = replica
            .histories
            .iter()
            .map(|history| (history.manifest.history.history_tree_id.as_str(), history))
            .collect::<BTreeMap<_, _>>();
        for group in &plan.groups {
            name_policy::validate_name(&group.directory)?;
            let directory = PathBuf::from(&group.directory);
            directories.insert(directory.clone());
            files.insert(directory.join("origin.json"), canonical_json(group)?);
            for (id, hash) in &group.histories {
                validate_id(id, "history")?;
                let history = histories[id.as_str()];
                let descriptor = HistoryDescriptor {
                    history_tree_id: id,
                    manifest_hash: hash,
                    availability: history
                        .availability
                        .iter()
                        .map(|(id, state)| (id.as_str(), Availability::from(*state)))
                        .collect(),
                };
                files.insert(
                    directory.join(format!("{id}.json")),
                    canonical_json(&descriptor)?,
                );
            }
        }
        let mut commitments = BTreeMap::new();
        for (path, bytes) in &files {
            if bytes.len() > MAX_METADATA {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "origin descriptor exceeds metadata limit",
                ));
            }
            commitments.insert(
                path.clone(),
                FileCommitment {
                    bytes: bytes.len() as u64,
                    digest: blake3::hash(bytes).to_hex().to_string(),
                },
            );
        }
        let inventory = Inventory {
            directories,
            files: commitments,
        };
        inventory.validate(&plan.budget)?;
        Ok(Self { files, inventory })
    }

    pub(super) fn install(&self, capsule: &DiskRoot, budget: &InspectionBudget) -> Result<()> {
        let path = capsule.path().join("origins");
        capsule.private_dir(&path)?;
        let root = capsule.private_child(&path)?;
        self.inventory
            .quarantine_residue(&root, capsule, "origins", budget)?;
        for directory in &self.inventory.directories {
            root.private_dir(&path.join(directory))?;
        }
        for (relative, bytes) in &self.files {
            super::transaction::ensure_exact(&root, &path.join(relative), bytes)?;
        }
        self.inventory.verify(&root, budget)?;
        self.inventory.sync(&root)
    }

    pub(super) fn verify_synced(
        &self,
        capsule: &DiskRoot,
        budget: &InspectionBudget,
    ) -> Result<()> {
        let root = capsule.private_child(&capsule.path().join("origins"))?;
        self.inventory.verify(&root, budget)?;
        self.inventory.sync(&root)
    }

    pub(super) fn verify(&self, capsule: &DiskRoot, budget: &InspectionBudget) -> Result<()> {
        let root = capsule.private_child(&capsule.path().join("origins"))?;
        self.inventory.verify(&root, budget)
    }
}
