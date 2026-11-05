//! Exact stop publication plans. Saved bytes restrict discovery; only a verified
//! Exit source or terminal SYSTEM archive permits replaying their write barriers.
use super::*;
use crate::engine::membership_transition::{
    pending::activation::exit::stop_plan::VerifiedExitStopSource,
    terminal::VerifiedTerminalAuthority,
};
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum StopFormat {
    #[serialization(rename = "prepared-ordinary-exit-stop")]
    Exit,
    #[serialization(rename = "prepared-terminal-exit-stop")]
    Terminal,
}

/// This is a bounded immutable plan, not an authorization token. Its optional
/// relocation and stop are persisted together before either ledger publication.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct PreparedStop {
    format: StopFormat,
    previous: Head,
    relocation: Option<Record>,
    stopped: Record,
    target: DiskRegistrationIdentity,
    operation_id: OperationId,
    intent_digest: IntentDigest,
}

/// A checked exact write set. Only real authority admission constructs it; it
/// cannot be deserialized or reused after a publication attempt consumes it.
pub(in crate::engine) struct AdmittedStopPublication<'a> {
    plan: &'a PreparedStop,
    stopped: LoadedLocator,
    current: Head,
}

impl BootstrapStore {
    pub(in crate::engine) fn prepare_exit_stop(
        &self,
        previous: &LoadedLocator,
        source: &VerifiedExitStopSource,
        observations: Vec<RootObservation>,
    ) -> Result<PreparedStop> {
        self.prepare_stop(
            previous,
            source.identity(),
            source.operation_id(),
            source.intent_digest(),
            StopFormat::Exit,
            observations,
        )
    }

    pub(in crate::engine) fn prepare_terminal_stop(
        &self,
        previous: &LoadedLocator,
        verified: &VerifiedTerminalAuthority,
    ) -> Result<PreparedStop> {
        let authority = &verified.authority;
        self.prepare_stop(
            previous,
            &authority.target(),
            authority.operation_id(),
            terminal_digest(verified)?,
            StopFormat::Terminal,
            Vec::new(),
        )
    }

    fn prepare_stop(
        &self,
        previous: &LoadedLocator,
        target: &DiskRegistrationIdentity,
        operation_id: OperationId,
        intent_digest: IntentDigest,
        format: StopFormat,
        observations: Vec<RootObservation>,
    ) -> Result<PreparedStop> {
        let current = self.reload(&previous.head.pool_id)?;
        if current.head != previous.head {
            return Err(BootstrapError::ManagementOnly("stop predecessor changed"));
        }
        let relocation = if matches!(format, StopFormat::Exit) && previous.record.stops.is_empty() {
            Record::extend(previous, observations)?
        } else {
            None
        };
        let relocated = relocation.as_ref().map(locator).transpose()?;
        let before_stop = relocated.as_ref().unwrap_or(previous);
        let stopped = stop_record(format, before_stop, target, operation_id, intent_digest)?;
        self.check_control_separation(&stopped)?;
        stopped.encode()?;
        Ok(PreparedStop {
            format,
            previous: previous.head.clone(),
            relocation,
            stopped,
            target: target.clone(),
            operation_id,
            intent_digest,
        })
    }

    /// Validate the exact chain and both publication windows before returning a
    /// probe selector that excludes the target, even if its stop head is pending.
    /// The selector supplies neither membership nor permission for normal I/O.
    pub(in crate::engine) fn stop_locator(&self, plan: &PreparedStop) -> Result<LoadedLocator> {
        let (stopped, _) = self.check_stop_plan(plan)?;
        Ok(stopped)
    }

    pub(in crate::engine) fn admit_exit_stop<'a>(
        &self,
        plan: &'a PreparedStop,
        source: &VerifiedExitStopSource,
    ) -> Result<AdmittedStopPublication<'a>> {
        if !matches!(plan.format, StopFormat::Exit)
            || &plan.target != source.identity()
            || plan.operation_id != source.operation_id()
            || plan.intent_digest != source.intent_digest()
        {
            return Err(BootstrapError::ManagementOnly(
                "stop plan differs from verified Exit authority",
            ));
        }
        let (selected, current) = self.check_stop_plan(plan)?;
        // A later readmission may legitimately change the current epoch. Bind
        // this authority to the exact historical stop node checked above, while
        // publication retains the latest authenticated locator and its head.
        let stopped = locator(&plan.stopped)?;
        if stopped.recorded_admission_epoch(&plan.target)?.as_deref() != source.admission_epoch() {
            return Err(BootstrapError::ManagementOnly(
                "stop plan closes another verified Exit admission epoch",
            ));
        }
        for observed in source.observations() {
            let expected = stopped
                .record
                .roots
                .iter()
                .find(|root| root.disk_id == observed.record.disk_id)
                .ok_or(BootstrapError::ManagementOnly(
                    "verified retained root is absent from stop plan",
                ))?;
            let mut record = observed.record.clone();
            record.metadata_member = expected.metadata_member;
            if observed.pool_id != stopped.record.pool_id || &record != expected {
                return Err(BootstrapError::ManagementOnly(
                    "stop plan differs from independently verified retained root",
                ));
            }
        }
        Ok(AdmittedStopPublication {
            plan,
            stopped: selected,
            current,
        })
    }

    pub(in crate::engine) fn admit_terminal_stop<'a>(
        &self,
        plan: &'a PreparedStop,
        verified: &VerifiedTerminalAuthority,
    ) -> Result<AdmittedStopPublication<'a>> {
        let authority = &verified.authority;
        if !matches!(plan.format, StopFormat::Terminal)
            || plan.target != authority.target()
            || plan.operation_id != authority.operation_id()
            || plan.intent_digest != terminal_digest(verified)?
        {
            return Err(BootstrapError::ManagementOnly(
                "stop plan differs from verified terminal authority",
            ));
        }
        let (selected, current) = self.check_stop_plan(plan)?;
        // Replaying an old Stop must prove its original lifetime, not reject
        // the separately certified epoch in a later readmission successor.
        let stopped = locator(&plan.stopped)?;
        let admission_epoch =
            authority
                .io_admission_epoch(&plan.target.disk_id)
                .map_err(|source| BootstrapError::Io {
                    stage: "verified terminal stop admission epoch",
                    source: std::io::Error::other(source),
                })?;
        if stopped.recorded_admission_epoch(&plan.target)? != admission_epoch {
            return Err(BootstrapError::ManagementOnly(
                "stop plan closes another verified terminal admission epoch",
            ));
        }
        Ok(AdmittedStopPublication {
            plan,
            stopped: selected,
            current,
        })
    }

    fn check_stop_plan(&self, plan: &PreparedStop) -> Result<(LoadedLocator, Head)> {
        Head::decode(&plan.previous.bytes()?)?;
        let previous = self.load_chain(&plan.previous)?;
        let relocated = match &plan.relocation {
            None => None,
            Some(record) => {
                if !matches!(plan.format, StopFormat::Exit) || !previous.record.stops.is_empty() {
                    return Err(BootstrapError::ManagementOnly(
                        "stop plan relocates an already restricted locator",
                    ));
                }
                let observations = record
                    .roots
                    .iter()
                    .cloned()
                    .map(|record| RootObservation {
                        pool_id: previous.record.pool_id.clone(),
                        record,
                    })
                    .collect();
                if Record::extend(&previous, observations)?.as_ref() != Some(record) {
                    return Err(BootstrapError::ManagementOnly(
                        "stop relocation changes the exact predecessor",
                    ));
                }
                self.check_control_separation(record)?;
                Some(locator(record)?)
            }
        };
        let before_stop = relocated.as_ref().unwrap_or(&previous);
        let expected = stop_record(
            plan.format,
            before_stop,
            &plan.target,
            plan.operation_id,
            plan.intent_digest,
        )?;
        if expected != plan.stopped {
            return Err(BootstrapError::ManagementOnly(
                "prepared stop changes roots or historical stops",
            ));
        }
        self.check_control_separation(&expected)?;
        let stopped = locator(&expected)?;
        let current = self.read_head()?.ok_or(BootstrapError::ManagementOnly(
            "stop publication lost its head",
        ))?;
        if current.generation > stopped.head.generation {
            let later = self.reload(&plan.target.pool_id)?;
            let stored =
                self.read_required(&record_name(stopped.head.generation)?, MAX_RECORD_BYTES)?;
            if stored != expected.encode()?.0 {
                return Err(BootstrapError::ManagementOnly(
                    "later locator lacks the exact prepared stop",
                ));
            }
            return Ok((later, current));
        }
        let nodes = [&previous, before_stop, &stopped];
        if !nodes.iter().any(|node| node.head == current) {
            return Err(BootstrapError::ManagementOnly(
                "stop publication has another current head",
            ));
        }
        let inventory = self.inventory()?;
        let installed = nodes
            .iter()
            .find(|node| expected_records(node.head.generation) == inventory)
            .ok_or(BootstrapError::ManagementOnly(
                "stop publication has unrelated or missing records",
            ))?;
        if installed.head.generation < current.generation
            || installed.head.generation
                > current
                    .generation
                    .checked_add(1)
                    .ok_or(BootstrapError::ManagementOnly("stop generation overflow"))?
        {
            return Err(BootstrapError::ManagementOnly(
                "stop publication skipped a head barrier",
            ));
        }
        // Authenticate every present planned immutable record, including a full
        // next record whose matching head did not survive the interruption.
        for node in [relocated.as_ref(), Some(&stopped)].into_iter().flatten() {
            if node.head.generation <= installed.head.generation {
                let stored =
                    self.read_required(&record_name(node.head.generation)?, MAX_RECORD_BYTES)?;
                if stored != node.record.encode()?.0 {
                    return Err(BootstrapError::ManagementOnly(
                        "installed stop record differs from its exact plan",
                    ));
                }
            }
        }
        Ok((stopped, current))
    }

    pub(in crate::engine) fn publish_stop(
        &mut self,
        admitted: AdmittedStopPublication<'_>,
    ) -> Result<LoadedLocator> {
        let AdmittedStopPublication {
            plan,
            stopped,
            current,
        } = admitted;
        if current.generation >= stopped.head.generation {
            self.sync_current(&stopped)?;
            return Ok(stopped);
        }
        let previous = match &plan.relocation {
            Some(record) => {
                let relocated = locator(record)?;
                if current == plan.previous {
                    self.publish(Some(&plan.previous), record.clone())?;
                }
                relocated.head
            }
            None => plan.previous.clone(),
        };
        let stopped = self.publish(Some(&previous), plan.stopped.clone())?;
        self.sync_current(&stopped)?;
        Ok(stopped)
    }
}

fn locator(record: &Record) -> Result<LoadedLocator> {
    let (_, head) = record.encode()?;
    Ok(LoadedLocator {
        head,
        record: record.clone(),
    })
}
fn stop_record(
    format: StopFormat,
    previous: &LoadedLocator,
    target: &DiskRegistrationIdentity,
    operation: OperationId,
    digest: IntentDigest,
) -> Result<Record> {
    match format {
        StopFormat::Exit => Record::stop(previous, target, operation, digest),
        StopFormat::Terminal => Record::terminal_stop(previous, target, operation, digest),
    }
}
fn terminal_digest(verified: &VerifiedTerminalAuthority) -> Result<IntentDigest> {
    let digest = verified
        .authority
        .transition_hash()
        .map_err(|source| BootstrapError::Io {
            stage: "verified terminal stop digest",
            source: std::io::Error::other(source),
        })?;
    IntentDigest::parse(&digest)
}
