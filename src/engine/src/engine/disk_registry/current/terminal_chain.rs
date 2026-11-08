//! Complete terminal-chain namespace admission from actual first Publication.
//! This returns a private frozen view, not an Engine install or writer capability.
use super::*;
use crate::engine::membership_transition::terminal_chain::PublishedProof;

impl PreparedCurrentNamespaces {
    pub(in crate::engine) fn admit_terminal_chain(
        self,
        source: &State,
        publication: &PublishedProof,
    ) -> Result<CurrentNamespaces> {
        let mut view = self.view;
        let published = publication.state();
        if &view.registry != publication.registry()
            || view.source_snapshot != publication.frozen_snapshot()
            || view.source_sequence != publication.frozen_sequence()
            || view.source_business_generation != source.business_generation
            || published.business_generation != source.business_generation
            || Some(published.sequence) != source.sequence.checked_add(1)
            || Some(published.topology_generation) != source.topology_generation.checked_add(1)
            || !view.matches_source(source)
            || !view.matches_source(published)
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal-chain Current differs from its complete checkpoint and actual first Publication",
            ));
        }
        // Terminal management increments sequence without a business snapshot.
        // Preserve the authenticated business-source boundary and inherited mask;
        // only the admission sequence moves to the actual first native slot.
        view.activation_sequence = published.sequence;
        Ok(view)
    }
}
