//! Activation and historical replay share the same exact publication admission.
use super::*;
use crate::engine::disk_lifecycle::AdmittedDiskExitPublication;

impl PreparedCurrentNamespaces {
    pub(in crate::engine) fn admit(
        self,
        source: &State,
        published: &State,
        snapshot: &str,
        publication: &AdmittedDiskExitPublication,
    ) -> Result<CurrentNamespaces> {
        let mut view = self.view;
        if &view.registry != publication.registry()
            || view.mask_hash.as_deref() != publication.occurrence_mask_hash()
            || view.source_snapshot != publication.source_snapshot()
            || view.source_sequence != publication.source_sequence()
            || view.source_business_generation != publication.source_business_generation()
            || source.sequence != view.source_sequence
            || source.business_generation != view.source_business_generation
            || snapshot != publication.published_snapshot()
            || published.sequence != publication.published_sequence()
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "current namespace admission differs from durable registry publication",
            ));
        }
        if !view.matches_source(source) || !view.matches_source(published) {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "registry activation changed a frozen namespace head",
            ));
        }
        view.activation_sequence = publication.published_sequence();
        Ok(view)
    }
}

impl Engine {
    pub(in crate::engine) fn install_current_namespaces(
        &mut self,
        prepared: PreparedCurrentNamespaces,
        publication: &AdmittedDiskExitPublication,
    ) -> Result<()> {
        if !publication.permits_engine_install() {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "historical namespace proof does not authorize live engine installation",
            ));
        }
        let generation = self.config.generation().ok_or_else(pending_publication)?;
        if generation.registry() != Some(publication.registry())
            || generation.first_sequence()? != publication.published_sequence()
            || generation.snapshot().as_deref() != Some(publication.source_snapshot())
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "namespace publication proof belongs to another active generation",
            ));
        }
        let snapshot = self.snapshot.as_deref().ok_or_else(pending_publication)?;
        let view = prepared.admit(&generation.base, &self.state, snapshot, publication)?;
        self.workspace_namespace_baselines.clear();
        self.named_namespaces.install_current(&mut self.current_namespaces, Some(Arc::new(view)));
        Ok(())
    }
}
