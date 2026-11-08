//! A new backup binding needs its entire lower chain, not historical Config
//! rows that have since been disabled or detached. Newly added ancestors are
//! covered by the same exact pending-root set as the rest of the addition.
use super::{Engine, TransitionIntent, ValidatedConfig, pending};
use crate::engine::disk_registry::{
    DiskRegistrationIdentity, RegistrationMode, VerifiedDiskRegistry,
};
use crate::{DiskConfig, Error, ErrorCode, Result};
use std::collections::{BTreeMap, BTreeSet};

/// ValidatedConfig already proves adjacent levels, unique slots and no cycle.
/// This traversal adds no single-successor assumption. It visits each selected
/// ancestor once, even when several future routes share that ancestor.
struct AdditionChain<'a> {
    original: &'a ValidatedConfig,
    ancestors: BTreeMap<&'a str, &'a DiskConfig>,
}

impl<'a> AdditionChain<'a> {
    fn new(original: &'a ValidatedConfig, target: &'a ValidatedConfig) -> Self {
        let originals = original
            .disks
            .iter()
            .map(|disk| (disk.disk_id.as_str(), disk))
            .collect::<BTreeMap<_, _>>();
        let slots = target
            .disks
            .iter()
            .map(|disk| (disk.slot_id.to_ascii_lowercase(), disk))
            .collect::<BTreeMap<_, _>>();
        let mut visited = BTreeSet::new();
        let mut ancestors = BTreeMap::new();
        for added in target
            .disks
            .iter()
            .filter(|disk| !originals.contains_key(disk.disk_id.as_str()))
        {
            let mut current = added;
            while visited.insert(current.disk_id.as_str()) {
                if let Some(original) = originals.get(current.disk_id.as_str()) {
                    ancestors.insert(original.disk_id.as_str(), *original);
                }
                let Some(upstream) = &current.upstream_slot_id else {
                    break;
                };
                current = slots
                    .get(&upstream.to_ascii_lowercase())
                    .expect("invariant: validated target contains every lower-level slot");
            }
        }
        Self {
            original,
            ancestors,
        }
    }

    fn require_current_registry(&self, engine: &Engine) -> Result<()> {
        let registrations = engine
            .registered_disks()
            .map(|disk| (disk.disk_id, disk))
            .collect::<BTreeMap<_, _>>();
        for disk in self.ancestors.values() {
            let registered = registrations
                .get(disk.disk_id.as_str())
                .ok_or_else(|| unavailable(disk, "is not a current registered binding"))?;
            if registered.pool_id != self.original.pool_id
                || registered.slot_id != disk.slot_id
                || registered.binding_generation != disk.binding_generation
                || registered.level != disk.level
                || registered.upstream_slot_id != disk.upstream_slot_id.as_deref()
                || registered.registration_mode != RegistrationMode::Enabled
            {
                return Err(unavailable(
                    disk,
                    "is not the exact enabled lower-level binding",
                ));
            }
        }
        Ok(())
    }

    fn require_held_roots(&self, engine: &Engine) -> Result<()> {
        for disk in self.ancestors.values() {
            let resource = engine.disk_resources.get(disk)?;
            pending::admission::verify_named_root(&resource.root)?;
            engine.config.verify_root_membership(&resource.root, disk)?;
            crate::engine::provenance::validate_owned_seal(
                &engine.disk_resources,
                disk,
                &engine.metadata_integrity,
            )?;
            pending::admission::verify_named_root(&resource.root)?;
        }
        Ok(())
    }
}

impl Engine {
    pub(super) fn require_addition_chain(&self, target: &ValidatedConfig) -> Result<()> {
        let chain = AdditionChain::new(&self.config, target);
        chain.require_current_registry(self)?;
        chain.require_held_roots(self)
    }

    /// Pure registration check is also used by verification of existing joint
    /// receipts. It does not turn a later physical outage into a new decision.
    pub(super) fn require_addition_chain_registry(&self, target: &ValidatedConfig) -> Result<()> {
        AdditionChain::new(&self.config, target).require_current_registry(self)
    }

    pub(super) fn require_pending_addition_chain(&self) -> Result<()> {
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "addition chain requires its original fence",
            )
        })?;
        if fence.intent.effect.is_none() {
            self.require_addition_chain(&fence.intent.target)?;
        }
        Ok(())
    }
}

/// Cold authority checks use the registry authenticated by this exact Add
/// decision. They never inspect today's roots or choose another source disk.
pub(super) fn verify_authority_chain(
    intent: &TransitionIntent,
    registry: Option<&VerifiedDiskRegistry>,
) -> Result<()> {
    let chain = AdditionChain::new(intent.origin.config(), &intent.target);
    if let Some(registry) = registry {
        for disk in chain.ancestors.values() {
            let identity = DiskRegistrationIdentity {
                pool_id: chain.original.pool_id.clone(),
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                binding_generation: disk.binding_generation,
            };
            if registry.registration(&identity)? != Some(RegistrationMode::Enabled) {
                return Err(unavailable(
                    disk,
                    "was not enabled in the authenticated Add registry",
                ));
            }
        }
    }
    // An Add before the first registry publication retains its original
    // all-enabled Config semantics. PortableAuthority independently rejects a
    // missing continuation after any registry-bearing predecessor.
    Ok(())
}

fn unavailable(disk: &DiskConfig, reason: &str) -> Error {
    Error::new(
        ErrorCode::ConfigConflict,
        format!(
            "complete backup chain: lower disk {} {reason}",
            disk.disk_id
        ),
    )
}
