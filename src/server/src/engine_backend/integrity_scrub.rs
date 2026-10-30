//! The managed job owns this backend across all off-lock work and final admission.
use super::{EngineBackend, current_protected_identity, error};
use crate::{
    ApiError,
    integrity_scrub::{IntegrityScrubPolicy, ScrubExecution},
};
use storage_engine::{IntegrityScrubCancellation, IntegrityScrubReport, IntegrityScrubSchedule};

impl EngineBackend {
    pub(crate) fn begin_integrity_scrub_schedule(
        &self,
        policy: IntegrityScrubPolicy,
    ) -> Result<IntegrityScrubSchedule, ApiError> {
        let interval = policy
            .interval()
            .expect("invariant: only a periodic job registers a schedule owner");
        crate::lock(&self.engine)
            .begin_integrity_scrub_schedule(interval, policy.max_age())
            .map_err(error)
    }
    pub(crate) fn run_integrity_scrub(
        &self,
        admission: ScrubExecution<'_>,
        cancellation: &IntegrityScrubCancellation,
        mut observe: impl FnMut(&IntegrityScrubReport, u64),
        read: impl FnMut(usize),
    ) -> Result<IntegrityScrubReport, ApiError> {
        let plan = {
            let engine = crate::lock(&self.engine);
            // Queued HTTP identity is revalidated at actual baseline admission.
            // Periodic scans are an explicit private host-policy capability.
            let identity = match &admission {
                ScrubExecution::Standalone(identity) => Some(*identity),
                ScrubExecution::Scheduled { identity, .. } => *identity,
            };
            if let Some(identity) = identity {
                current_protected_identity(&engine, identity)?;
            }
            match admission {
                ScrubExecution::Standalone(_) => engine.prepare_integrity_scrub(),
                ScrubExecution::Scheduled { owner, .. } => {
                    engine.prepare_scheduled_integrity_scrub(owner)
                }
            }
            .map_err(error)?
        };
        let expected_payload_bytes = match plan.estimated_payload_bytes(cancellation) {
            Ok(bytes) => Some(bytes),
            Err(cause) if cause.code() == storage_engine::ErrorCode::OperationAborted => None,
            Err(cause) => return Err(error(cause)),
        };
        // An interrupted estimate has no total to publish. The same one-way
        // token makes run return Cancelled before storage reads; retain that
        // report and the ordinary live-owner finish instead of rejecting it.
        let outcome = plan.run(
            cancellation,
            |plan, version| crate::lock(&self.engine).admit_integrity_scrub_batch(plan, version),
            read,
            |report| {
                if let Some(bytes) = expected_payload_bytes {
                    observe(report, bytes);
                }
            },
        );
        Ok(crate::lock(&self.engine).finish_integrity_scrub(outcome))
    }
}
