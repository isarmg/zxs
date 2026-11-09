//! No-clobber identity preparation on each original lower root. A parent-side
//! completion prevents a missing finished mapping from becoming a new write.
use super::*;
use crate::foreign_import::identity_closure::{
    ForeignIdentityClosureReport, ForeignIdentityTarget, MAX_MAPPING_BYTES, Prepared,
};
use transaction::ensure_exact;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForeignIdentityClosureStatus {
    NotStarted,
    Preparing,
    Verified {
        report: Box<ForeignIdentityClosureReport>,
    },
}
#[derive(Serialize)]
struct Binding<'a> {
    format: &'static str,
    parent: PhysicalBinding,
    capsule_digest: String,
    report: &'a ForeignIdentityClosureReport,
}
struct Material {
    prepared: Prepared,
    binding: Vec<u8>,
    intent: PathBuf,
    mapping: PathBuf,
    complete: PathBuf,
}
impl ForeignMigrationTransaction {
    /// Independently derive and verify the complete naming plan from this
    /// target's own original native State and verified three-scope images.
    pub fn preview_identity_closure(
        &self,
        target: &ForeignIdentityTarget,
    ) -> Result<(ForeignIdentityClosureReport, ForeignIdentityClosureStatus)> {
        let material = Material::capture(self, target)?;
        let status = material.status(self)?;
        Ok((material.prepared.report, status))
    }
    /// Material only. The caller retains the current Engine exclusion/auth owner
    /// and rechecks its baseline before/after this call. No target ID becomes live.
    pub fn prepare_identity_closure(
        &self,
        target: &ForeignIdentityTarget,
    ) -> Result<ForeignIdentityClosureReport> {
        let material = Material::capture(self, target)?;
        material.status(self)?;
        ensure_exact(&self.parent, &material.intent, &material.binding)?;
        ensure_exact(&self.parent, &material.mapping, &material.prepared.bytes)?;
        ensure_exact(&self.parent, &material.complete, &material.binding)?;
        self.parent.sync_dir(self.parent.path())?;
        if material.status(self)?
            != (ForeignIdentityClosureStatus::Verified {
                report: Box::new(material.prepared.report.clone()),
            })
        {
            return Err(changed("foreign object identity material did not complete"));
        }
        Ok(material.prepared.report)
    }
}
impl Material {
    fn capture(
        transaction: &ForeignMigrationTransaction,
        target: &ForeignIdentityTarget,
    ) -> Result<Self> {
        transaction.check_names()?;
        let Placement::ChainMaterial {
            chain_digest,
            target_level,
        } = &transaction.intent.plan.placement
        else {
            return Err(invalid(
                "object identity closure requires the entire original chain",
            ));
        };
        if !transaction.published
            || transaction.intent.plan.operation_id != target.baseline.operation_id
            || *chain_digest != target.baseline.chain_digest
        {
            return Err(changed(
                "object identity closure differs from the original whole-chain operation",
            ));
        }
        target.require_lower(
            transaction.parent.path(),
            *target_level,
            &transaction.intent.plan.source_path,
        )?;
        let ForeignDatabaseStatus::Verified { report } = transaction.original_database_status()?
        else {
            return Err(invalid(
                "original three-scope databases must independently verify before object identity preparation",
            ));
        };
        let source = transaction.verify_replica()?;
        let prepared = Prepared::capture(target, &source)?;
        if prepared.report.original != report.original {
            return Err(changed(
                "native identity closure and original database State disagree",
            ));
        }
        let binding = canonical_json(&Binding {
            format: "zxs-foreign-object-identity-binding",
            parent: PhysicalBinding::of(&transaction.parent)?,
            capsule_digest: transaction.digest.to_string(),
            report: &prepared.report,
        })?;
        let prefix = format!(
            ".zxs-foreign-identities-{}",
            transaction.intent.plan.operation_id
        );
        Ok(Self {
            prepared,
            binding,
            intent: transaction
                .parent
                .path()
                .join(format!("{prefix}-intent.json")),
            mapping: transaction.parent.path().join(format!("{prefix}.json")),
            complete: transaction
                .parent
                .path()
                .join(format!("{prefix}-complete.json")),
        })
    }
    fn status(
        &self,
        transaction: &ForeignMigrationTransaction,
    ) -> Result<ForeignIdentityClosureStatus> {
        let intent = self.exact(&transaction.parent, &self.intent, &self.binding)?;
        let mapping = self.exact(&transaction.parent, &self.mapping, &self.prepared.bytes)?;
        let complete = self.exact(&transaction.parent, &self.complete, &self.binding)?;
        transaction.check_names()?;
        if complete {
            if !intent || !mapping {
                return Err(changed(
                    "completed foreign identity material lost its original intent or mapping; no recreation",
                ));
            }
            return Ok(ForeignIdentityClosureStatus::Verified {
                report: Box::new(self.prepared.report.clone()),
            });
        }
        if !intent {
            if mapping {
                return Err(changed(
                    "foreign identity mapping lacks its original reservation",
                ));
            }
            return Ok(ForeignIdentityClosureStatus::NotStarted);
        }
        Ok(ForeignIdentityClosureStatus::Preparing)
    }
    fn exact(&self, root: &DiskRoot, path: &Path, expected: &[u8]) -> Result<bool> {
        let Some(file) = root.open_optional_read_regular(path)? else {
            return Ok(false);
        };
        if read_file_bytes(file, MAX_MAPPING_BYTES)? != expected {
            return Err(changed(
                "foreign identity material differs from its own native source or current pool baseline",
            ));
        }
        Ok(true)
    }
}
