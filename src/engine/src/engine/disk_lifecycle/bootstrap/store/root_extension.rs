//! Exact replay of an explicitly prepared root expansion. Ordinary reload stays
//! strict: it never elects an orphan record or treats a saved plan as authority.
use super::*;
use data_serialization::{Deserialize, Serialize};
mod terminal_chain;
pub(in crate::engine) use terminal_chain::ChainRootPlanInfo;

#[derive(Clone, Debug, Serialize, Deserialize)]
enum ExtensionFormat {
    #[serialization(rename = "prepared-bootstrap-root-extension")]
    Prepared,
}

/// Serializable intent, not a permission token. Resumption additionally checks
/// the immutable prior chain and fresh independently authorized observations.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PreparedRootExtension {
    format: ExtensionFormat,
    previous: Head,
    next: Record,
}

impl BootstrapStore {
    /// The caller must durably store this exact plan in its protected operation
    /// before attempting the ledger update. This call mutates no external state.
    pub(crate) fn prepare_root_extension(
        &self,
        previous: &LoadedLocator,
        roots: Vec<RootObservation>,
    ) -> Result<Option<PreparedRootExtension>> {
        let current = self.reload(&previous.head.pool_id)?;
        if current.head != previous.head {
            return Err(BootstrapError::ManagementOnly(
                "root extension predecessor changed",
            ));
        }
        Record::extend(previous, roots)?
            .map(|next| {
                self.check_control_separation(&next)?;
                next.encode()?;
                Ok(PreparedRootExtension {
                    format: ExtensionFormat::Prepared,
                    previous: previous.head.clone(),
                    next,
                })
            })
            .transpose()
    }

    /// Return a restrictive probe selector, never registration authority. The
    /// caller must bind the plan to its protected operation before using it,
    /// authenticate actual member evidence, and pass fresh observations to replay.
    pub(crate) fn root_extension_locator(
        &self,
        plan: &PreparedRootExtension,
    ) -> Result<LoadedLocator> {
        let previous = self.load_chain(&plan.previous)?;
        let observations = plan
            .next
            .roots
            .iter()
            .cloned()
            .map(|record| RootObservation {
                pool_id: plan.next.pool_id.clone(),
                record,
            })
            .collect();
        let expected = Record::extend(&previous, observations)?.ok_or(
            BootstrapError::ManagementOnly("prepared root expansion is empty"),
        )?;
        if expected != plan.next {
            return Err(BootstrapError::ManagementOnly(
                "prepared root expansion changes its prior stops or history",
            ));
        }
        self.check_control_separation(&expected)?;
        let (bytes, head) = expected.encode()?;
        let current = self.read_head()?;
        if current.as_ref() != Some(&plan.previous) && current.as_ref() != Some(&head) {
            return Err(BootstrapError::ManagementOnly(
                "root expansion selector has another current head",
            ));
        }
        let inventory = self.inventory()?;
        let before = expected_records(plan.previous.generation);
        let after = expected_records(head.generation);
        if inventory != before && inventory != after {
            return Err(BootstrapError::ManagementOnly(
                "root expansion selector has unrelated or missing records",
            ));
        }
        if inventory == after {
            let stored = self.read_required(&record_name(head.generation)?, MAX_RECORD_BYTES)?;
            if stored != bytes {
                return Err(BootstrapError::ManagementOnly(
                    "root expansion selector differs from exact installed record",
                ));
            }
        } else if current.as_ref() == Some(&head) {
            return Err(BootstrapError::ManagementOnly(
                "root expansion head has no corresponding immutable record",
            ));
        }
        Ok(LoadedLocator {
            head,
            record: expected,
        })
    }

    /// Completion callers may verify an exact installed head, never promote a
    /// merely planned/orphan successor by treating a SYSTEM marker as authority.
    pub(in crate::engine) fn require_published_root_extension(
        &self,
        plan: &PreparedRootExtension,
    ) -> Result<()> {
        let expected = self.root_extension_locator(plan)?;
        let current = self.reload(&plan.previous.pool_id)?;
        if current.head != expected.head || current.record != expected.record {
            return Err(BootstrapError::ManagementOnly(
                "completed root extension lacks its exact installed head",
            ));
        }
        self.sync_current(&current)
    }

    /// Replays only this one exact successor. The supplied observations must
    /// come from the caller's independently authenticated operation/held roots.
    /// A serialized plan or a matching caller-supplied hash is never enough.
    pub(crate) fn resume_root_extension(
        &mut self,
        plan: &PreparedRootExtension,
        roots: Vec<RootObservation>,
    ) -> Result<LoadedLocator> {
        let previous = self.load_chain(&plan.previous)?;
        let expected = Record::extend(&previous, roots)?.ok_or(BootstrapError::ManagementOnly(
            "root extension no longer changes its exact predecessor",
        ))?;
        self.publish_root_extension(plan, expected)
    }

    fn publish_root_extension(
        &mut self,
        plan: &PreparedRootExtension,
        expected: Record,
    ) -> Result<LoadedLocator> {
        if expected != plan.next {
            return Err(BootstrapError::ManagementOnly(
                "root extension differs from authenticated observations",
            ));
        }
        let (_, next_head) = expected.encode()?;
        let current = self.read_head()?.ok_or(BootstrapError::ManagementOnly(
            "root extension lost its head",
        ))?;
        if current.generation > next_head.generation {
            // Later legitimate additions/stops may follow this consumed plan.
            // Verify the named exact node within the current immutable chain,
            // never overwrite a newer head or erase a later stop.
            let current = self.reload(&plan.previous.pool_id)?;
            let stored =
                self.read_required(&record_name(next_head.generation)?, MAX_RECORD_BYTES)?;
            if Record::decode(&stored)? != expected {
                return Err(BootstrapError::ManagementOnly(
                    "later locator does not contain the exact root extension",
                ));
            }
            self.sync_current(&current)?;
            return Ok(current);
        }
        // publish already admits only previous/next heads and the exact before
        // or after inventory. A full next record with the old head is resumable;
        // unrelated records, missing history and unknown staging stay closed.
        let current = self.publish(Some(&plan.previous), expected)?;
        self.sync_current(&current)?;
        Ok(current)
    }
}

use crate::engine::membership_transition::{
    terminal::VerifiedTerminalAuthority, terminal_resume::ResumeCore,
};

impl BootstrapStore {
    /// A terminal record cannot enter the ordinary Add path. This separate lane
    /// consumes a completed old authority token and the independently verified
    /// full target certificate, while retaining every historical stopped root.
    pub(in crate::engine) fn prepare_terminal_root_extension(
        &self,
        previous: &LoadedLocator,
        observations: Vec<RootObservation>,
        terminal: &VerifiedTerminalAuthority,
        core: &ResumeCore,
    ) -> crate::Result<PreparedRootExtension> {
        let current = self
            .reload(&previous.head.pool_id)
            .map_err(runtime::management_error)?;
        if current.head != previous.head {
            return Err(crate::Error::new(
                crate::ErrorCode::RecoveryRequired,
                "terminal root extension predecessor changed",
            ));
        }
        let expected = authenticated_terminal_extension(previous, observations, terminal, core)?;
        self.check_control_separation(&expected)
            .map_err(runtime::management_error)?;
        expected.encode().map_err(runtime::management_error)?;
        Ok(PreparedRootExtension {
            format: ExtensionFormat::Prepared,
            previous: previous.head.clone(),
            next: expected,
        })
    }

    pub(in crate::engine) fn resume_terminal_root_extension(
        &mut self,
        plan: &PreparedRootExtension,
        observations: Vec<RootObservation>,
        terminal: &VerifiedTerminalAuthority,
        core: &ResumeCore,
    ) -> crate::Result<LoadedLocator> {
        let previous = self
            .load_chain(&plan.previous)
            .map_err(runtime::management_error)?;
        let expected = authenticated_terminal_extension(&previous, observations, terminal, core)?;
        self.publish_root_extension(plan, expected)
            .map_err(runtime::management_error)
    }
}

fn authenticated_terminal_extension(
    previous: &LoadedLocator,
    observations: Vec<RootObservation>,
    terminal: &VerifiedTerminalAuthority,
    core: &ResumeCore,
) -> crate::Result<Record> {
    let target = terminal.authority.target();
    let digest = terminal.authority.transition_hash()?;
    let epoch = terminal.authority.io_admission_epoch(&target.disk_id)?;
    if !previous.stop_hints().any(|stop| {
        stop.disk_id == target.disk_id
            && stop.slot_id == target.slot_id
            && stop.binding_generation == target.binding_generation
            && stop.operation_id == terminal.authority.operation_id()
            && stop.intent_digest.to_string() == digest
            && stop.admission_epoch.map(|value| value.to_string()) == epoch
    }) {
        return Err(crate::Error::new(
            crate::ErrorCode::PublicationInvalid,
            "terminal expansion lacks the exact completed stop",
        ));
    }
    let selected = RootObservation::from_terminal_resume(terminal, core)?;
    if let Some((request, operation, transition)) = core.retained_event()? {
        let expected = Record::resume_terminal_readmission(
            previous,
            selected,
            request,
            operation,
            &transition,
        )
        .map_err(runtime::management_error)?;
        let observed = Record::resume_terminal_readmission(
            previous,
            observations,
            request,
            operation,
            &transition,
        )
        .map_err(runtime::management_error)?;
        if observed != expected {
            return Err(crate::Error::new(
                crate::ErrorCode::DiskIdentityMismatch,
                "terminal readmission differs from its independently certified root identity",
            ));
        }
        return Ok(expected);
    }
    let expected =
        Record::resume_terminal(previous, selected).map_err(runtime::management_error)?;
    let observed =
        Record::resume_terminal(previous, observations).map_err(runtime::management_error)?;
    if observed != expected {
        return Err(crate::Error::new(
            crate::ErrorCode::DiskIdentityMismatch,
            "terminal expansion differs from independently certified target identities",
        ));
    }
    Ok(expected)
}

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/engine/bootstrap_root_extension_tests.rs"]
mod tests;
