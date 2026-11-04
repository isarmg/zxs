//! Terminal stop retries read only protected SYSTEM state. Full archive
//! authentication remains TerminalStore::load_authority's responsibility.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::{
    LoadedLocator,
    store::{BootstrapStore, PreparedStop},
};

const STOP_PLAN: &str = "bootstrap-stop.json";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct TerminalStopPlan {
    authority_hash: String,
    stop: PreparedStop,
}
impl TerminalStore {
    pub(in crate::engine::disk_lifecycle::terminal) fn synchronize_stop_plan(
        &self,
        bootstrap: &mut BootstrapStore,
        verified: &VerifiedTerminalAuthority,
        live_binding: Option<&DiskIoBinding>,
    ) -> Result<LoadedLocator> {
        if verified.authority.hash()? != self.head.authority_hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal stop uses another verified SYSTEM authority",
            ));
        }
        self.verify_stop_inventory()?;
        let path = self.operation.path().join(STOP_PLAN);
        let plan = match optional::<TerminalStopPlan>(&self.operation, &path)? {
            Some(plan) => {
                if plan.authority_hash != self.head.authority_hash {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "terminal stop plan differs from its exact authority",
                    ));
                }
                plan
            }
            None => {
                // Missing plans do not adopt a dangling record. Strict reload
                // rejects an interrupted update without its original plan.
                let previous = bootstrap
                    .reload(&self.head.pool_id)
                    .map_err(management_error)?;
                let identity = verified.authority.target();
                let digest = IntentDigest::parse(&verified.authority.transition_hash()?)
                    .map_err(management_error)?;
                if previous.stop_hints().any(|stop| {
                    stop.disk_id == identity.disk_id
                        && stop.slot_id == identity.slot_id
                        && stop.binding_generation == identity.binding_generation
                        && stop.operation_id == verified.authority.operation_id()
                        && stop.intent_digest == digest
                }) {
                    bootstrap
                        .sync_current(&previous)
                        .map_err(management_error)?;
                    return Ok(previous);
                }
                TerminalStopPlan {
                    authority_hash: self.head.authority_hash.clone(),
                    stop: bootstrap
                        .prepare_terminal_stop(&previous, verified)
                        .map_err(management_error)?,
                }
            }
        };
        let admitted = bootstrap
            .admit_terminal_stop(&plan.stop, verified)
            .map_err(management_error)?;
        // The original plan and SYSTEM head are durable before the first stop
        // record. Identical retries re-establish both file/directory barriers.
        self.operation.write_json(&path, &plan, false)?;
        self.directory
            .write_json(&self.directory.path().join(HEAD), &self.head, false)?;
        match bootstrap.publish_stop(admitted).map_err(management_error) {
            Ok(locator) => Ok(locator),
            Err(error) => {
                if let Some(binding) = live_binding {
                    drop(binding.begin_quiesce().map_err(|quiesce| {
                        quiesce.context(
                            ErrorCode::RecoveryRequired,
                            format!(
                                "cannot quiesce after uncertain terminal stop publication: {error}"
                            ),
                        )
                    })?);
                }
                Err(error)
            }
        }
    }

    fn verify_stop_inventory(&self) -> Result<()> {
        for entry in self.operation.entries(self.operation.path())? {
            let entry = entry.map_err(crate::disk::fs_error)?;
            let Some(name) = entry.as_os_str().to_str() else {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "non-UTF-8 terminal stop entry",
                ));
            };
            if !matches!(
                name,
                AUTHORITY
                    | COMPLETE
                    | STOP_PLAN
                    | "resume.json"
                    | "resume-complete.json"
                    | "resume-generation.json"
                    | "resume-bootstrap.json"
            ) {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "unknown or unfinished terminal stop write",
                ));
            }
        }
        Ok(())
    }
}
