//! One new admission event over the terminal locator. Prior stops, root names,
//! identities and all earlier admission events remain byte-for-byte retained.
use super::*;
use crate::engine::membership_transition::readmission::ReadmissionRequest;

impl Record {
    pub(in crate::engine::disk_lifecycle::bootstrap) fn resume_terminal_readmission(
        previous: &LoadedLocator,
        observations: Vec<RootObservation>,
        request: &ReadmissionRequest,
        operation: OperationId,
        transition: &str,
    ) -> Result<Self> {
        let source = &previous.record;
        if !source.is_terminal() {
            return Err(BootstrapError::ManagementOnly(
                "retained terminal admission has no terminal source",
            ));
        }
        let root = source
            .roots
            .iter()
            .find(|root| root.disk_id == request.target.disk_id)
            .ok_or(BootstrapError::ManagementOnly(
                "retained terminal directory was not recorded",
            ))?;
        if source.pool_id != request.target.pool_id
            || root.slot_id != request.target.slot_id
            || root.binding_generation != request.target.binding_generation
            || root.device != request.directory.device
            || root.inode != request.directory.inode
            || !root.metadata_member
            || source
                .admission_epoch(&root.disk_id)
                .map(|value| value.to_string())
                != request.previous_admission
            || !source.stops.iter().any(|stop| {
                stop.disk_id == root.disk_id
                    && stop.operation_id == request.stopped_operation
                    && stop.intent_digest.to_string() == request.stopped_transition
                    && stop.admission_epoch == source.admission_epoch(&root.disk_id)
            })
        {
            return Err(BootstrapError::ManagementOnly(
                "terminal readmission differs from the exact stopped root lifetime",
            ));
        }
        let mut selected = false;
        let mut seen = std::collections::BTreeSet::new();
        for observation in observations {
            let observed = observation.record;
            let existing = source
                .roots
                .iter()
                .find(|root| root.disk_id == observed.disk_id)
                .ok_or(BootstrapError::ManagementOnly(
                    "retained terminal admission cannot introduce roots",
                ))?;
            if observation.pool_id != source.pool_id
                || &observed != existing
                || !seen.insert(observed.disk_id.clone())
                || (source.is_stopped(&observed.disk_id)
                    && observed.disk_id != request.target.disk_id)
            {
                return Err(BootstrapError::ManagementOnly(
                    "terminal readmission observations change or open another stopped registration",
                ));
            }
            selected |= observed.disk_id == request.target.disk_id;
        }
        if !selected {
            return Err(BootstrapError::ManagementOnly(
                "terminal readmission omits its exact retained metadata root",
            ));
        }
        let mut record = source.clone();
        record.format = RecordFormat::RestrictiveLocator;
        record.generation =
            record
                .generation
                .checked_add(1)
                .ok_or(BootstrapError::ManagementOnly(
                    "locator generation overflow",
                ))?;
        record.previous = Some(previous.head.clone());
        record.readmissions.push(ReadmissionRecord {
            disk_id: request.target.disk_id.clone(),
            operation_id: operation,
            intent_digest: IntentDigest::parse(transition)?,
            stopped_intent: IntentDigest::parse(&request.stopped_transition)?,
        });
        record.retained = record
            .roots
            .iter()
            .filter(|root| root.metadata_member && !record.is_stopped(&root.disk_id))
            .map(|root| root.disk_id.clone())
            .collect();
        record.validate()?;
        Ok(record)
    }
}
