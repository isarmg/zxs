//! A sealed, exact restart plan for one additional admission event. It cannot
//! authorize discovery until the readmission's independent witnesses are checked.
use super::*;
use crate::engine::membership_transition::readmission::AdmittedReadmission;
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct PreparedReadmission {
    previous: Head,
    next: Record,
}

impl BootstrapStore {
    pub(in crate::engine) fn prepare_readmission(
        &self,
        previous: &LoadedLocator,
        admission: &AdmittedReadmission,
    ) -> Result<PreparedReadmission> {
        let next = Record::readmit(previous, admission)?;
        next.follows(previous)?;
        Ok(PreparedReadmission {
            previous: previous.head.clone(),
            next,
        })
    }
}

impl BootstrapStore {
    pub(in crate::engine) fn readmission_source(
        &self,
        plan: &PreparedReadmission,
    ) -> Result<LoadedLocator> {
        let previous = self.load_chain(&plan.previous)?;
        plan.next.follows(&previous)?;
        let (_, next) = plan.next.encode()?;
        let current = self.read_head()?;
        if current.as_ref() != Some(&plan.previous) && current.as_ref() != Some(&next) {
            return Err(BootstrapError::ManagementOnly(
                "readmission plan is not the current exact locator transition",
            ));
        }
        let inventory = self.inventory()?;
        if inventory != expected_records(plan.previous.generation)
            && inventory != expected_records(next.generation)
        {
            return Err(BootstrapError::ManagementOnly(
                "readmission plan does not cover locator inventory",
            ));
        }
        Ok(previous)
    }
    pub(in crate::engine) fn resume_readmission(
        &mut self,
        plan: &PreparedReadmission,
        admission: &AdmittedReadmission,
    ) -> Result<LoadedLocator> {
        let previous = self.readmission_source(plan)?;
        if Record::readmit(&previous, admission)? != plan.next {
            return Err(BootstrapError::ManagementOnly(
                "readmission plan differs from verified authority",
            ));
        }
        self.publish(Some(&plan.previous), plan.next.clone())
    }
}

impl BootstrapStore {
    /// Reads protected SYSTEM certificates only, before any storage-root stat.
    pub(in crate::engine) fn readmission_probe_admissions(
        &self,
        locator: &LoadedLocator,
    ) -> crate::Result<
        Vec<crate::engine::membership_transition::readmission::ReadmissionProbeAdmission>,
    > {
        let system = self.directory.path().parent().ok_or_else(|| {
            crate::Error::new(
                crate::ErrorCode::RecoveryRequired,
                "bootstrap control has no SYSTEM parent",
            )
        })?;
        locator
            .record
            .readmission_events()
            .filter(|event| {
                !locator.record.is_stopped(&event.disk_id)
                    && locator.record.admission_epoch(&event.disk_id) == Some(event.intent_digest)
            })
            .map(|event| {
                crate::engine::membership_transition::readmission::ReadmissionProbeAdmission::read(
                    system,
                    &event.intent_digest.to_string(),
                )
            })
            .collect()
    }
}

impl BootstrapStore {
    /// Only fresh SYSTEM recovery may seed already published epochs. Existing
    /// ledgers must use their immutable stop/readmission append protocol instead.
    pub(in crate::engine::disk_lifecycle::bootstrap) fn initialize_recovered_readmissions(
        &mut self,
        pool_id: &str,
        roots: Vec<RootObservation>,
        recovered: &[crate::engine::membership_transition::readmission::RecoveredReadmission],
    ) -> Result<LoadedLocator> {
        let mut record = Record::initial(pool_id, roots)?;
        for admission in recovered {
            let request = admission.request();
            let root = record
                .roots
                .iter()
                .find(|root| root.disk_id == request.target.disk_id)
                .ok_or(BootstrapError::ManagementOnly(
                    "recovered epoch has no observed root",
                ))?;
            if pool_id != request.target.pool_id
                || root.slot_id != request.target.slot_id
                || root.binding_generation != request.target.binding_generation
                || root.device != request.directory.device
                || root.inode != request.directory.inode
                || Path::new(&root.path) != admission.root()
            {
                return Err(BootstrapError::ManagementOnly(
                    "recovered epoch differs from held recovery identity",
                ));
            }
            let event = super::super::format::ReadmissionRecord {
                disk_id: root.disk_id.clone(),
                operation_id: admission.operation(),
                intent_digest: IntentDigest::parse(admission.transition())?,
                stopped_intent: IntentDigest::parse(&request.stopped_transition)?,
            };
            if record
                .recovered_readmissions
                .insert(root.disk_id.clone(), event)
                .is_some()
            {
                return Err(BootstrapError::ManagementOnly("duplicate recovered epoch"));
            }
        }
        self.publish(None, record)
    }
}
