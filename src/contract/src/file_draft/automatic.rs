//! Description of an explicitly enabled automatic publication controller.
//! Only the original SYSTEM save/commit decision may advance this state.
use super::WorkingRevision;
use crate::{FileNodeId, Generation, OperationId, Sequence, Timestamp};
use data_serialization::{Deserialize, Deserializer, Serialize};

pub const WINDOW_SECONDS: u64 = 60;
pub const MAX_SAVES: usize = 20;

/// Strictly one supported, persisted policy, including in enable requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum AutomaticPolicy {
    TwentySavesInSixtySeconds,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SaveDecision {
    pub revision: WorkingRevision,
    pub operation: OperationId,
    pub sequence: Sequence,
    pub server_time: Timestamp,
    pub effective_time: Timestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ProtectionReason { SaveFrequency }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ProtectionTrigger {
    pub reason: ProtectionReason,
    pub epoch: Generation,
    decision: SaveDecision,
}
impl ProtectionTrigger {
    pub const fn decision(&self) -> &SaveDecision { &self.decision }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AutomaticNodeDecision {
    pub epoch: Generation,
    pub save: SaveDecision,
    pub node: FileNodeId,
    pub operation: OperationId,
    pub sequence: Sequence,
    pub published_at: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum Window {
    Armed {
        #[serialization(deserialize_with = "bounded_samples")]
        recent: Vec<SaveDecision>,
    },
    Protected {
        trigger: ProtectionTrigger,
        #[serialization(deserialize_with = "bounded_samples")]
        triggering_samples: Vec<SaveDecision>,
    },
}

fn bounded_samples<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<SaveDecision>, D::Error> {
    struct Samples;
    impl<'de> data_serialization::de::Visitor<'de> for Samples {
        type Value = Vec<SaveDecision>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 21 original successful save decisions")
        }
        fn visit_seq<A: data_serialization::de::SeqAccess<'de>>(self, mut sequence: A)
            -> Result<Self::Value, A::Error>
        {
            let mut samples = Vec::new();
            while let Some(sample) = sequence.next_element()? {
                if samples.len() > MAX_SAVES {
                    return Err(data_serialization::de::Error::custom("too many automatic saves"));
                }
                samples.push(sample);
            }
            Ok(samples)
        }
    }
    d.deserialize_seq(Samples)
}

/// Pending is one newest eligible save in this epoch, never a history backlog.
/// A physical FULL not yet decided is not a published node or a backup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AutomaticController {
    policy: AutomaticPolicy,
    epoch: Generation,
    watermark: Timestamp,
    #[serialization(deserialize_with = "required_option")]
    latest: Option<SaveDecision>,
    #[serialization(deserialize_with = "required_option")]
    pending: Option<SaveDecision>,
    #[serialization(deserialize_with = "required_option")]
    last_node: Option<AutomaticNodeDecision>,
    window: Window,
}

fn required_option<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where D: Deserializer<'de>, T: Deserialize<'de> {
    Option::<T>::deserialize(d)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
pub enum ControllerError {
    #[error("automatic controller has an inconsistent policy, epoch, phase or saved window")]
    Invalid,
    #[error("the automatic save is not a new ordered original decision")]
    Order,
    #[error("the automatic controller epoch, saved revision or protection latch changed")]
    Fence,
}

impl AutomaticController {
    pub fn enabled(revision: WorkingRevision, policy: AutomaticPolicy) -> Self {
        Self {
            epoch: revision.generation(), policy, watermark: Timestamp::from_unix_seconds(0),
            latest: None, pending: None, last_node: None, window: Window::Armed { recent: Vec::new() },
        }
    }

    pub const fn epoch(&self) -> Generation { self.epoch }
    pub const fn policy(&self) -> AutomaticPolicy { self.policy }
    pub const fn latest_save(&self) -> Option<&SaveDecision> { self.latest.as_ref() }
    pub const fn pending_save(&self) -> Option<&SaveDecision> { self.pending.as_ref() }
    pub const fn last_node(&self) -> Option<&AutomaticNodeDecision> { self.last_node.as_ref() }
    pub const fn protection(&self) -> Option<&ProtectionTrigger> {
        match &self.window { Window::Armed { .. } => None, Window::Protected { trigger, .. } => Some(trigger) }
    }

    pub fn validate(&self, working: WorkingRevision) -> Result<(), ControllerError> {
        let invalid = ControllerError::Invalid;
        if self.epoch.get() == 0 || self.epoch > working.generation() { return Err(invalid); }
        if let Some(latest) = self.latest
            && (latest.revision > working || latest.sequence.get() == 0
                || latest.effective_time != self.watermark || latest.server_time > latest.effective_time)
        { return Err(invalid); }
        if self.latest.is_none() && self.watermark.unix_seconds() != 0 { return Err(invalid); }
        let samples = match &self.window {
            Window::Armed { recent } => {
                if recent.len() > MAX_SAVES || (!recent.is_empty() && recent.last() != self.latest.as_ref())
                    || self.pending.is_some_and(|save| Some(save) != self.latest || recent.last() != Some(&save))
                { return Err(invalid); }
                recent
            }
            Window::Protected { trigger, triggering_samples } => {
                if triggering_samples.len() != MAX_SAVES + 1 || trigger.epoch != self.epoch
                    || triggering_samples.last() != Some(&trigger.decision) || self.pending.is_some()
                    || self.latest.is_none_or(|save| save.revision < trigger.decision.revision
                        || save.sequence < trigger.decision.sequence)
                { return Err(invalid); }
                triggering_samples
            }
        };
        if let Some(newest) = samples.last() {
            let mut ids = std::collections::BTreeSet::new();
            for (i, sample) in samples.iter().enumerate() {
                if !ids.insert(sample.operation) || sample.revision.generation() <= self.epoch
                    || sample.sequence.get() == 0 || sample.revision > working
                    || sample.server_time > sample.effective_time
                    || newest.effective_time.unix_seconds().checked_sub(sample.effective_time.unix_seconds())
                        .is_none_or(|age| age >= WINDOW_SECONDS)
                    || (i > 0 && (samples[i-1].revision >= sample.revision
                        || samples[i-1].sequence >= sample.sequence
                        || samples[i-1].effective_time > sample.effective_time))
                { return Err(invalid); }
            }
        }
        if let Some(node) = self.last_node
            && (node.epoch > self.epoch || node.epoch >= node.save.revision.generation()
                || node.save.sequence >= node.sequence || node.operation == node.save.operation
                || self.latest.is_none_or(|save| save.revision < node.save.revision))
        { return Err(invalid); }
        Ok(())
    }

    /// Called only by the original successful save decision. An existing receipt
    /// retries before this function; duplicate operations cannot mint samples.
    pub fn saved(&self, revision: WorkingRevision, operation: OperationId,
        sequence: Sequence, server_time: Timestamp) -> Result<Self, ControllerError>
    {
        if revision.generation() <= self.epoch || sequence.get() == 0
            || self.latest.is_some_and(|old| old.revision >= revision || old.sequence >= sequence
                || old.operation == operation)
        { return Err(ControllerError::Order); }
        let mut after = self.clone();
        let effective_time = server_time.max(self.watermark);
        let save = SaveDecision { revision, operation, sequence, server_time, effective_time };
        after.watermark = effective_time;
        after.latest = Some(save);
        if let Window::Armed { recent } = &mut after.window {
            recent.retain(|item| effective_time.unix_seconds()
                .saturating_sub(item.effective_time.unix_seconds()) < WINDOW_SECONDS);
            recent.push(save);
            if recent.len() > MAX_SAVES {
                let samples = std::mem::take(recent);
                after.window = Window::Protected {
                    trigger: ProtectionTrigger { reason: ProtectionReason::SaveFrequency,
                        epoch: self.epoch, decision: save },
                    triggering_samples: samples,
                };
                after.pending = None;
            } else { after.pending = Some(save); }
        }
        after.validate(revision)?;
        Ok(after)
    }

    pub fn resumed(&self, expected_epoch: Generation, revision: WorkingRevision)
        -> Result<Self, ControllerError>
    {
        if self.epoch != expected_epoch || self.protection().is_none()
            || revision.generation() <= self.epoch
            || self.latest.is_some_and(|latest| latest.revision >= revision)
        { return Err(ControllerError::Fence); }
        let mut after = self.clone();
        after.epoch = revision.generation();
        after.pending = None;
        after.window = Window::Armed { recent: Vec::new() };
        after.validate(revision)?;
        Ok(after)
    }

    /// A node can only be recorded for the exact newest still-eligible save.
    /// This is a logical CAS, not authority to create any physical artifact.
    pub fn published(&self, node: AutomaticNodeDecision, revision: WorkingRevision)
        -> Result<Self, ControllerError>
    {
        if self.epoch != node.epoch || self.pending != Some(node.save) || self.protection().is_some() {
            return Err(ControllerError::Fence);
        }
        let mut after = self.clone();
        after.pending = None;
        after.last_node = Some(node);
        after.validate(revision)?;
        Ok(after)
    }

    /// An explicit manual node may consume the latest body while remaining in
    /// automatic/protected mode. It never becomes an automatic publication.
    pub fn consumed_manually(&self) -> Self {
        let mut after = self.clone();
        after.pending = None;
        after
    }
}

#[cfg(test)]
#[path = "../../../../test/contract/unit/automatic_controller.rs"]
mod tests;
