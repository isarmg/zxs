//! No-clobber source-scoped graph preparation, after exact identity completion.
//! This is not a target database, State or membership installation.
use super::*;
use crate::foreign_import::identity_closure::{
    ForeignHistoryGraphReport, ForeignIdentityTarget, PreparedGraph,
};
use transaction::ensure_exact;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForeignHistoryGraphStatus {
    NotStarted,
    Preparing,
    Verified { report: ForeignHistoryGraphReport },
}
#[derive(Serialize)]
struct Binding<'a> {
    format: &'static str,
    parent: PhysicalBinding,
    capsule_digest: String,
    report: &'a ForeignHistoryGraphReport,
}
struct Material {
    prepared: PreparedGraph,
    binding: Vec<u8>,
    intent: PathBuf,
    graph: PathBuf,
    complete: PathBuf,
}
impl ForeignMigrationTransaction {
    /// Independently derive and verify the complete mapped history graph from this
    /// target's own original native State and verified three-scope images.
    pub fn preview_history_graph(
        &self,
        target: &ForeignIdentityTarget,
    ) -> Result<(ForeignHistoryGraphReport, ForeignHistoryGraphStatus)> {
        let material = Material::capture(self, target)?;
        let status = material.status(self)?;
        Ok((material.prepared.report, status))
    }
    /// Material only. The caller retains the current Engine exclusion/auth owner
    /// and rechecks its baseline before/after this call. No target ID becomes live.
    /// Source slots, paths, audit actors and publication sequences remain
    /// explicitly source-scoped.
    pub fn prepare_history_graph(
        &self,
        target: &ForeignIdentityTarget,
    ) -> Result<ForeignHistoryGraphReport> {
        let material = Material::capture(self, target)?;
        material.status(self)?;
        ensure_exact(&self.parent, &material.intent, &material.binding)?;
        ensure_exact(&self.parent, &material.graph, &material.prepared.bytes)?;
        ensure_exact(&self.parent, &material.complete, &material.binding)?;
        self.parent.sync_dir(self.parent.path())?;
        if material.status(self)?
            != (ForeignHistoryGraphStatus::Verified {
                report: material.prepared.report.clone(),
            })
        {
            return Err(changed(
                "foreign source-scoped history graph material did not complete",
            ));
        }
        Ok(material.prepared.report)
    }
}
impl Material {
    fn capture(
        transaction: &ForeignMigrationTransaction,
        target: &ForeignIdentityTarget,
    ) -> Result<Self> {
        let (identities, identity_status) = transaction.preview_identity_closure(target)?;
        if identity_status
            != (ForeignIdentityClosureStatus::Verified {
                report: identities.clone(),
            })
        {
            return Err(invalid(
                "exact object identity material must complete before graph preparation",
            ));
        }
        let source = transaction.verify_replica()?;
        let prepared = PreparedGraph::capture(target, &source)?;
        if prepared.report.identities != identities {
            return Err(changed(
                "native graph, original identity material and database State disagree",
            ));
        }
        let binding = canonical_json(&Binding {
            format: "zxs-foreign-source-scoped-graph-binding",
            parent: PhysicalBinding::of(&transaction.parent)?,
            capsule_digest: transaction.digest.to_string(),
            report: &prepared.report,
        })?;
        let prefix = format!(
            ".zxs-foreign-graphs-{}",
            transaction.intent.plan.operation_id
        );
        Ok(Self {
            prepared,
            binding,
            intent: transaction
                .parent
                .path()
                .join(format!("{prefix}-intent.json")),
            graph: transaction.parent.path().join(format!("{prefix}.json")),
            complete: transaction
                .parent
                .path()
                .join(format!("{prefix}-complete.json")),
        })
    }
    fn status(
        &self,
        transaction: &ForeignMigrationTransaction,
    ) -> Result<ForeignHistoryGraphStatus> {
        let intent = self.exact(&transaction.parent, &self.intent, &self.binding)?;
        let graph = self.exact(&transaction.parent, &self.graph, &self.prepared.bytes)?;
        let complete = self.exact(&transaction.parent, &self.complete, &self.binding)?;
        transaction.check_names()?;
        if complete {
            if !intent || !graph {
                return Err(changed(
                    "completed foreign graph material lost its original intent or graph; no recreation",
                ));
            }
            return Ok(ForeignHistoryGraphStatus::Verified {
                report: self.prepared.report.clone(),
            });
        }
        if !intent {
            if graph {
                return Err(changed(
                    "foreign mapped graph lacks its original reservation",
                ));
            }
            return Ok(ForeignHistoryGraphStatus::NotStarted);
        }
        Ok(ForeignHistoryGraphStatus::Preparing)
    }
    fn exact(&self, root: &DiskRoot, path: &Path, expected: &[u8]) -> Result<bool> {
        let Some(file) = root.open_optional_read_regular(path)? else {
            return Ok(false);
        };
        if read_file_bytes(file, expected.len())? != expected {
            return Err(changed(
                "foreign mapped graph differs from its own native source or current pool baseline",
            ));
        }
        Ok(true)
    }
}
