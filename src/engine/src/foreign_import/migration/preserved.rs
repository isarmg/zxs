//! Observations for the SYSTEM-only preserved-chain record. A digest alone
//! cannot issue these: the source's complete native prefix and real inventory
//! are taken from the named, held source. No routine here changes a root.
use super::*;
use crate::persist::layout;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PreservedMaterial {
    Source {
        original_registration: DiskRegistrationIdentity,
        source_evidence_digest: ForeignEvidenceDigest,
        source_inventory_hash: InventoryHash,
        #[serialization(with = "storage_contract::u64_string")]
        seal_byte_length: u64,
        seal_bytes_digest: ContentHash,
        #[serialization(with = "storage_contract::u64_string")]
        journal_byte_length: u64,
        journal_bytes_digest: ContentHash,
    },
    Lower {
        capsule_plan_digest: ForeignEvidenceDigest,
        capsule_inventory_hash: InventoryHash,
        original_scopes_hash: OriginalScopesHash,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(transparent)]
pub(crate) struct InventoryHash(ForeignEvidenceDigest);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(transparent)]
pub(crate) struct OriginalScopesHash(ForeignEvidenceDigest);

fn canonical_digest(kind: &str, value: &impl Serialize) -> Result<ForeignEvidenceDigest> {
    object_hash(kind, value)?.parse().map_err(|e| Error::from(std::io::Error::other(e)))
}

impl Inventory {
    fn preservation_hash(&self) -> Result<InventoryHash> {
        canonical_digest("foreign-preserved-inventory", self).map(InventoryHash)
    }

    fn observed_file(&self, path: &Path) -> Result<(u64, ContentHash)> {
        let file = self.files.get(path).ok_or_else(|| changed("missing original control file"))?;
        let digest = file.digest.parse().map_err(|e| Error::from(std::io::Error::other(e)))?;
        Ok((file.bytes, digest))
    }
}

pub(crate) struct PreservedSourceInventory<'a> {
    source: &'a ForeignImportPreview,
    inventory: Inventory,
    budget: InspectionBudget,
    original: ForeignOriginalStateReport,
    material: PreservedMaterial,
}

impl<'a> PreservedSourceInventory<'a> {
    pub(crate) fn capture(source: &'a ForeignImportPreview, selected: &DiskRoot) -> Result<Self> {
        if selected.path() != source.root.path()
            || PhysicalBinding::of(selected)? != PhysicalBinding::of(&source.root)?
        {
            return Err(changed("selected highest root is not the held original source"));
        }
        let refreshed = ForeignImportPreview::inspect(
            selected.directory(selected.path())?, selected.path().into(), source.limits,
        )?;
        if refreshed.digest != source.digest || refreshed.source != source.source {
            return Err(changed("original Foreign evidence changed before capture"));
        }
        let (original, _) = refreshed.visit_original_states(|_, _, _, _| Ok(()))?;
        if !original.registrations.contains(&source.source.registration) {
            return Err(changed("original State does not own the selected source registration"));
        }
        let budget = InspectionBudget::from_limits(source.limits);
        let inventory = Inventory::capture(selected, &budget)?;
        inventory.validate(&budget)?;
        let (seal_byte_length, seal_bytes_digest) = inventory.observed_file(layout::DISK_IDENTITY.as_path())?;
        let journal = refreshed.journal_path.strip_prefix(selected.path())
            .map_err(|_| changed("original journal is outside the selected source"))?;
        let (journal_byte_length, journal_bytes_digest) = inventory.observed_file(journal)?;
        if seal_byte_length != refreshed.original_seal().len() as u64
            || seal_bytes_digest.digest() != blake3::hash(refreshed.original_seal())
            || ForeignEvidenceDigest::from_digest(journal_bytes_digest.digest()) != refreshed.journal_digest
        {
            return Err(changed("actual inventory differs from original raw seal or journal"));
        }
        let material = PreservedMaterial::Source {
            original_registration: source.source.registration.clone(),
            source_evidence_digest: source.digest,
            source_inventory_hash: inventory.preservation_hash()?,
            seal_byte_length,
            seal_bytes_digest,
            journal_byte_length,
            journal_bytes_digest,
        };
        source.revalidate_source()?;
        Ok(Self { source, inventory, budget, original, material })
    }

    pub(crate) fn original(&self) -> &ForeignOriginalStateReport { &self.original }
    pub(crate) fn material(&self) -> &PreservedMaterial { &self.material }

    pub(crate) fn lower_plan(
        &self, operation: OperationId, chain: &ForeignEvidenceDigest, level: u32,
    ) -> Result<ForeignMigrationPlan> {
        ForeignMigrationPlan::from_inventory(
            self.source, operation,
            Placement::ChainMaterial { chain_digest: chain.to_string(), target_level: level },
            None, self.inventory.clone(), self.budget.clone(),
        )
    }

    pub(crate) fn recheck(&self) -> Result<()> {
        self.inventory.verify(&self.source.root, &self.budget)?;
        self.source.revalidate_source()
    }

    pub(crate) fn capture_lower(
        &self, owner: &ForeignMigrationTransaction, expected: &ForeignMigrationPlan,
    ) -> Result<PreservedMaterial> {
        owner.check_names()?;
        if !owner.published || owner.digest != expected.digest || owner.intent.plan != expected.record
            || owner.intent.plan.inventory != self.inventory
        {
            return Err(changed("lower capsule does not bind this exact original native plan"));
        }
        owner.require_portable_intent()?;
        // This independently replays this capsule, verifies its copied source
        // and projection, then compares every local original scope database.
        let ForeignDatabaseStatus::Verified { report } = owner.original_database_status()? else {
            return Err(changed("lower lacks its own completed original scope images"));
        };
        let Placement::ChainMaterial { target_level, chain_digest } = &expected.record.placement else {
            return Err(changed("a preserved lower is not ordinary migration material"));
        };
        if report.original != self.original || report.target_level != *target_level
            || report.capsule_digest != owner.digest.to_string() || &report.chain_digest != chain_digest
        {
            return Err(changed("lower original scope report differs from the selected source or role"));
        }
        // Include the actual complete capsule directory, not just its `source`
        // subtree or a copy of the high source's report. Keep existing ceilings.
        let actual = Inventory::capture(&owner.capsule, &owner.intent.plan.budget)?;
        actual.validate(&owner.intent.plan.budget)?;
        let material = PreservedMaterial::Lower {
            capsule_plan_digest: owner.digest,
            capsule_inventory_hash: actual.preservation_hash()?,
            original_scopes_hash: OriginalScopesHash(canonical_digest(
                "foreign-preserved-original-scopes", &*report,
            )?),
        };
        owner.check_names()?;
        Ok(material)
    }
}
