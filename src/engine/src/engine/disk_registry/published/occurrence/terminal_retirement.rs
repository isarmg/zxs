//! A terminal namespace is authenticated preparation, not an active read view.
//! Each exact management successor removes occurrences from that preparation;
//! it never reconstructs earlier removals from unfiltered historical entries.
use super::*;
use crate::engine::disk_registry::{
    current::PreparedCurrentNamespaces, published::terminal::TerminalRegistry,
};

impl<'a> FrozenDetachSource<'a> {
    pub(in crate::engine) fn capture_terminal(
        state: &'a crate::State,
        config: &'a crate::ValidatedConfig,
        registry: &'a TerminalRegistry,
        prior: &'a PreparedCurrentNamespaces,
        target: &DiskRegistrationIdentity,
    ) -> ProjectionResult<Self> {
        let verified = registry.verified_registry();
        let view = &prior.view;
        let source = &verified.state.source;
        if !verified.state.voters.is_empty()
            || verified.state.pending_exit.is_none()
            || source.pool_id != config.pool_id
            || source.sequence != state.sequence
            || source.topology_generation != state.topology_generation
            || source.snapshot_hash.as_deref() != Some(view.source_snapshot.as_str())
            || view.source_sequence != state.sequence
            || view.source_business_generation != state.business_generation
            || view.registry != *registry.commitment()
            || view.registry_state.state != verified.state
            || !view.matches_source(state)
        {
            return Err(ProjectionError::SourceMismatch(
                "terminal retirement lacks its exact verified predecessor namespace",
            ));
        }
        // Config is the independently authenticated historical topology, not a
        // replacement voter set or permission to probe any recorded root.
        Self::capture_verified(
            state,
            config,
            source.snapshot_hash.as_deref(),
            verified,
            target,
            Some(view),
        )
    }
}

impl VerifiedOccurrenceMask<'_, '_> {
    pub(in crate::engine) fn prepare_terminal_retirement_namespaces(
        &self,
        successor: &TerminalRegistry,
    ) -> ProjectionResult<PreparedCurrentNamespaces> {
        let next = successor.verified_registry();
        let prior = self.source.registry;
        let detached = next
            .state
            .detached
            .get(&self.state.binding.target.disk_id)
            .ok_or(ProjectionError::BaselineMismatch)?;
        if !prior.state.voters.is_empty()
            || !next.state.voters.is_empty()
            || next.state.pending_exit != prior.state.pending_exit
            || next.state.source != self.state.binding.source
            || next.state.predecessor.as_ref() != Some(prior.commitment())
            || Some(next.commitment.view_generation())
                != prior.commitment.view_generation().checked_add(1)
            || detached.predecessor != *prior.commitment()
            || detached.view_generation != next.commitment.view_generation()
            || detached.request.target != self.state.binding.target
            || !matches!(
                &detached.request.effect,
                RegistryExitEffect::Detach { occurrence_mask_hash } if occurrence_mask_hash == &self.hash
            )
        {
            return Err(ProjectionError::BaselineMismatch);
        }
        self.source
            .own_current_namespaces(next, Some(&self.hash), Some(&self.state.scopes))
    }
}
