//! Fresh payload observations are not durable health or repairability permits.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ObservedCopyState {
    CompleteVerified,
    Missing,
    Corrupt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrozenCopyState {
    CompleteVerified,
    Unavailable,
}

impl FrozenCopyState {
    fn parse(value: &Value) -> Result<Self> {
        match value.as_str() {
            Some("COMPLETE_VERIFIED") => Ok(Self::CompleteVerified),
            Some("UNAVAILABLE") => Ok(Self::Unavailable),
            _ => Err(invalid_protection()),
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Self::CompleteVerified => "COMPLETE_VERIFIED",
            Self::Unavailable => "UNAVAILABLE",
        }
    }
}

#[derive(Debug)]
pub(super) struct SlotIdentity {
    pub slot_id: String,
    pub copy_level: u32,
    pub disk_id: String,
    pub binding_generation: u64,
}

#[derive(Debug)]
pub(super) struct VersionPlacementObservation {
    pub version_id: String,
    pub primary_slot_id: String,
    pub target_copies: u32,
    pub required_slots: Vec<String>,
    pub topology_generation: u64,
}

#[derive(Debug)]
pub(super) struct ObservedSlot {
    pub identity: SlotIdentity,
    pub state: ObservedCopyState,
}

#[derive(Debug)]
pub(super) struct ObservedVersion {
    pub placement: VersionPlacementObservation,
    pub slots: Vec<ObservedSlot>,
}

struct FrozenSlot {
    identity: SlotIdentity,
    state: FrozenCopyState,
}

struct FrozenVersion {
    placement: VersionPlacementObservation,
    slots: Vec<FrozenSlot>,
}

enum LossCause {
    Missing,
    Corrupt,
}

struct LossDetail {
    version_id: String,
    slot_id: String,
    cause: LossCause,
}

pub(super) struct FrozenObservation {
    versions: Vec<FrozenVersion>,
    losses: Vec<LossDetail>,
}

/// A projection of frozen evidence, never a claim about current payload bytes.
pub(crate) struct VerifiedPayloads(BTreeMap<String, BTreeSet<String>>);

impl VerifiedPayloads {
    pub(crate) fn contains(&self, version: &str, slot: &str) -> bool {
        self.0
            .get(version)
            .is_some_and(|slots| slots.contains(slot))
    }

    pub(super) fn matches_receipts(&self, receipts: &[PayloadReceipt]) -> bool {
        let actual = receipts
            .iter()
            .map(|r| (r.version_id.as_str(), r.slot_id.as_str()))
            .collect::<BTreeSet<_>>();
        actual.len() == receipts.len()
            && actual.len() == self.0.values().map(BTreeSet::len).sum::<usize>()
            && actual
                .iter()
                .all(|(version, slot)| self.contains(version, slot))
    }
}

impl FrozenObservation {
    pub(super) fn freeze(observed: Vec<ObservedVersion>) -> Self {
        let mut losses = Vec::new();
        let versions = observed
            .into_iter()
            .map(|version| {
                let slots = version
                    .slots
                    .into_iter()
                    .map(|slot| {
                        let cause = match slot.state {
                            ObservedCopyState::CompleteVerified => None,
                            ObservedCopyState::Missing => Some(LossCause::Missing),
                            ObservedCopyState::Corrupt => Some(LossCause::Corrupt),
                        };
                        let state = if let Some(cause) = cause {
                            losses.push(LossDetail {
                                version_id: version.placement.version_id.clone(),
                                slot_id: slot.identity.slot_id.clone(),
                                cause,
                            });
                            FrozenCopyState::Unavailable
                        } else {
                            FrozenCopyState::CompleteVerified
                        };
                        FrozenSlot {
                            identity: slot.identity,
                            state,
                        }
                    })
                    .collect();
                FrozenVersion {
                    placement: version.placement,
                    slots,
                }
            })
            .collect();
        Self { versions, losses }
    }

    pub(super) fn version_is_complete(&self, version: &str) -> Option<bool> {
        self.versions
            .iter()
            .find(|v| v.placement.version_id == version)
            .map(|v| {
                v.slots
                    .iter()
                    .all(|s| s.state == FrozenCopyState::CompleteVerified)
            })
    }

    pub(super) fn current_health(&self) -> storage_contract::CurrentHealth {
        if self.versions.iter().any(|v| {
            !v.slots
                .iter()
                .any(|s| s.state == FrozenCopyState::CompleteVerified)
        }) {
            storage_contract::CurrentHealth::Unavailable
        } else {
            storage_contract::CurrentHealth::Degraded
        }
    }

    pub(super) fn verified_payloads(&self) -> VerifiedPayloads {
        let mut expected = BTreeMap::<String, BTreeSet<String>>::new();
        for version in &self.versions {
            let std::collections::btree_map::Entry::Vacant(entry) =
                expected.entry(version.placement.version_id.clone())
            else {
                continue;
            };
            let slots = entry.insert(BTreeSet::new());
            for slot in &version.slots {
                if slot.state == FrozenCopyState::CompleteVerified {
                    slots.insert(slot.identity.slot_id.clone());
                }
            }
        }
        VerifiedPayloads(expected)
    }

    /// Preserve the existing recovery-plan projection, including array order
    /// and string-valued generations. External plans never pass through this.
    pub(super) fn wire(&self) -> (Vec<Value>, Vec<Value>) {
        let versions = self
            .versions
            .iter()
            .map(|v| {
                let slots = v
                    .slots
                    .iter()
                    .map(|s| {
                        let SlotIdentity {
                            slot_id,
                            copy_level,
                            disk_id,
                            binding_generation,
                        } = &s.identity;
                        json!({"slot_id":slot_id,"copy_level":copy_level,"disk_id":disk_id,
                    "binding_generation":binding_generation.to_string(),"state":s.state.wire()})
                    })
                    .collect::<Vec<_>>();
                let VersionPlacementObservation {
                    version_id,
                    primary_slot_id,
                    target_copies,
                    required_slots,
                    topology_generation,
                } = &v.placement;
                json!({"version_id":version_id,"primary_slot_id":primary_slot_id,
                "target_copies":target_copies,"required_slots":required_slots,
                "topology_generation":topology_generation.to_string(),"slots":slots})
            })
            .collect();
        let losses = self
            .losses
            .iter()
            .map(|loss| {
                let cause = match loss.cause {
                    LossCause::Missing => "MISSING",
                    LossCause::Corrupt => "CORRUPT",
                };
                json!({"version_id":loss.version_id,"slot_id":loss.slot_id,"cause":cause})
            })
            .collect();
        (versions, losses)
    }
}

fn invalid_protection() -> Error {
    Error::new(
        storage_contract::ErrorCode::PublicationInvalid,
        "degraded publication recovery evidence is invalid",
    )
}

/// Admit only the fields consumed by proof verification, at its existing
/// boundary. All other nested JSON remains exact hash-bound evidence.
pub(super) fn admit_verified_payloads(
    plan: &RecoveryPlan,
    c: &Candidate,
) -> Result<VerifiedPayloads> {
    if plan.version_protection.len() != c.versions.len() {
        return Err(invalid_protection());
    }
    let mut expected = BTreeMap::<String, BTreeSet<String>>::new();
    let mut below = false;
    for version in &c.versions {
        let observation = plan
            .version_protection
            .iter()
            .find(|v| v["version_id"].as_str() == Some(version.version.version_id.as_str()))
            .ok_or_else(invalid_protection)?;
        let slots = observation["slots"]
            .as_array()
            .ok_or_else(invalid_protection)?;
        if slots.len() != version.placement.route_slot_ids.len()
            || observation["required_slots"] != json!(version.placement.route_slot_ids)
            || observation["target_copies"] != json!(version.placement.target_copies)
            || observation["primary_slot_id"] != json!(version.placement.primary_slot_id)
        {
            return Err(invalid_protection());
        }
        let mut slot_set = BTreeSet::new();
        for slot in slots {
            let id = slot["slot_id"].as_str().ok_or_else(invalid_protection)?;
            if !slot_set.insert(id) || !version.placement.route_slot_ids.iter().any(|s| s == id) {
                return Err(invalid_protection());
            }
            match FrozenCopyState::parse(&slot["state"])? {
                FrozenCopyState::CompleteVerified => {
                    expected
                        .entry(version.version.version_id.clone())
                        .or_default()
                        .insert(id.to_string());
                }
                FrozenCopyState::Unavailable => below = true,
            }
        }
    }
    if !below {
        return Err(invalid_protection());
    }
    Ok(VerifiedPayloads(expected))
}
