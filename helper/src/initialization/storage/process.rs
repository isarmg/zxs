//! Adapters for the shared helper-only privilege-separated process owner.
use super::{WorkerOutput, WorkerRequest};
use crate::Result;
impl crate::service_worker::Request for WorkerRequest {
    type Output = WorkerOutput;
    const COMMAND: &'static str = "internal-storage-worker";
    fn account(&self) -> &crate::account::Snapshot {
        &self.context.account
    }
    fn execute(self) -> Result<Self::Output> {
        super::worker::execute(self)
    }
}
pub(super) fn run(request: WorkerRequest) -> Result<WorkerOutput> {
    crate::service_worker::run(request)
}
pub(crate) fn entry() -> Result<()> {
    crate::service_worker::entry::<WorkerRequest>()
}
