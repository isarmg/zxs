//! Raw locator writes for storage/discovery fixtures only. Production stop
//! publication requires independently authenticated authority and an exact plan.
use super::*;

impl BootstrapStore {
    /// Unauthenticated storage fixture; never exported in production builds.
    pub(crate) fn append_roots(
        &mut self,
        previous: &LoadedLocator,
        roots: Vec<RootObservation>,
    ) -> Result<LoadedLocator> {
        match Record::extend(previous, roots)? {
            Some(record) => self.publish(Some(&previous.head), record),
            None => {
                self.sync_current(previous)?;
                Ok(previous.clone())
            }
        }
    }

    /// Exercise raw atomic record/head barriers without manufacturing an
    /// admitted production Exit. Discovery must independently verify authority.
    pub(crate) fn append_stop(
        &mut self,
        previous: &LoadedLocator,
        target: &DiskRegistrationIdentity,
        operation_id: OperationId,
        intent_digest: IntentDigest,
    ) -> Result<LoadedLocator> {
        let record = Record::stop(previous, target, operation_id, intent_digest)?;
        self.publish(Some(&previous.head), record)
    }
}
