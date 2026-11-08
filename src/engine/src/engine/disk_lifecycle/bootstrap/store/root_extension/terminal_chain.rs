//! Exact future locator record for a fully published terminal chain. Preparation
//! and inspection never call publish/sync_current or grant a probing capability.
use super::*;
use crate::engine::membership_transition::terminal_chain::ChainEpochSource;

pub(in crate::engine) struct ChainRootPlanInfo {
    pub(in crate::engine) previous_generation: u64,
    pub(in crate::engine) previous_hash: String,
    pub(in crate::engine) next_generation: u64,
    pub(in crate::engine) next_hash: String,
}
impl BootstrapStore {
    pub(in crate::engine) fn prepare_chain_root_extension(
        &self,
        source: &ChainEpochSource<'_>,
        actual_target: RootObservation,
    ) -> crate::Result<PreparedRootExtension> {
        let previous = self
            .reload(&source.request().target.pool_id)
            .map_err(runtime::management_error)?;
        let expected = chain_record(&previous, source)?;
        let actual = Record::resume_terminal_readmission(
            &previous,
            vec![actual_target],
            source.request(),
            source.operation(),
            source.epoch(),
        )
        .map_err(runtime::management_error)?;
        if actual != expected {
            return Err(crate::Error::new(
                crate::ErrorCode::DiskIdentityMismatch,
                "chain activation plan does not retain the exact independently observed original root",
            ));
        }
        self.check_control_separation(&expected)
            .map_err(runtime::management_error)?;
        expected.encode().map_err(runtime::management_error)?;
        Ok(PreparedRootExtension {
            format: ExtensionFormat::Prepared,
            previous: previous.head,
            next: expected,
        })
    }
    /// Historical plan inspection is deliberately not a locator: no caller may
    /// use it to probe the future unstopped set or construct an I/O binding.
    pub(in crate::engine) fn inspect_chain_root_extension(
        &self,
        plan: &PreparedRootExtension,
        source: &ChainEpochSource<'_>,
    ) -> crate::Result<ChainRootPlanInfo> {
        let previous = self
            .reload(&source.request().target.pool_id)
            .map_err(runtime::management_error)?;
        if previous.head != plan.previous {
            return Err(crate::Error::new(
                crate::ErrorCode::RecoveryRequired,
                "unconsumed chain activation plan lost its exact current locator predecessor",
            ));
        }
        let expected = chain_record(&previous, source)?;
        if expected != plan.next {
            return Err(crate::Error::new(
                crate::ErrorCode::PublicationInvalid,
                "chain activation plan changes roots, prior stops, admissions or original selected lifetime",
            ));
        }
        self.check_control_separation(&expected)
            .map_err(runtime::management_error)?;
        let (_, next) = expected.encode().map_err(runtime::management_error)?;
        Ok(ChainRootPlanInfo {
            previous_generation: previous.head.generation,
            previous_hash: previous.head.record_hash.to_string(),
            next_generation: next.generation,
            next_hash: next.record_hash.to_string(),
        })
    }
}
fn chain_record(previous: &LoadedLocator, source: &ChainEpochSource<'_>) -> crate::Result<Record> {
    let authority = source.terminal();
    let target = authority.target();
    let transition = authority.transition_hash()?;
    let epoch = authority.io_admission_epoch(&target.disk_id)?;
    if !previous.stop_hints().any(|stop| {
        stop.disk_id == target.disk_id
            && stop.slot_id == target.slot_id
            && stop.binding_generation == target.binding_generation
            && stop.operation_id == authority.operation_id()
            && stop.intent_digest.to_string() == transition
            && stop.admission_epoch.map(|value| value.to_string()) == epoch
    }) {
        return Err(crate::Error::new(
            crate::ErrorCode::PublicationInvalid,
            "chain activation plan lacks the exact original terminal Stop",
        ));
    }
    let request = source.request();
    let disk = source.disk();
    let record = RootRecord {
        disk_id: request.target.disk_id.clone(),
        slot_id: request.target.slot_id.clone(),
        binding_generation: request.target.binding_generation,
        path: canonical_root(&disk.root).map_err(runtime::management_error)?,
        device: request.directory.device,
        inode: request.directory.inode,
        metadata_member: true,
    };
    record.validate().map_err(runtime::management_error)?;
    Record::resume_terminal_readmission(
        previous,
        vec![RootObservation {
            pool_id: request.target.pool_id.clone(),
            record,
        }],
        request,
        source.operation(),
        source.epoch(),
    )
    .map_err(runtime::management_error)
}
