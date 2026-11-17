//! Reconstruct inherited namespace admission from exact frozen publications,
//! never from a raw current head or a serialized view supplied by a directory.
use super::*;
use crate::engine::disk_registry::current::CurrentNamespaces;
use crate::engine::membership_transition::archive::persistent::namespace_replay::{
    VerifiedArchivePublication, replay_installed_publications,
};

pub(in crate::engine::membership_transition) struct NamespaceReplay {
    state: Arc<State>,
    snapshot: Option<String>,
    view: Option<CurrentNamespaces>,
}
impl NamespaceReplay {
    pub(in crate::engine::membership_transition) fn new() -> Self {
        Self {
            state: Arc::new(State {
                topology_generation: 1,
                ..State::default()
            }),
            snapshot: None,
            view: None,
        }
    }
    pub(in crate::engine::membership_transition) fn advance(
        &mut self,
        publication: &VerifiedArchivePublication,
    ) -> Result<()> {
        let candidate = publication.candidate();
        if candidate.previous_snapshot != self.snapshot {
            return Err(Error::new(
                ErrorCode::LogFork,
                "namespace replay does not extend its exact source snapshot",
            ));
        }
        if let Some(authority) = publication.activation() {
            super::super::verify_frozen_activation(authority, Arc::clone(&self.state), candidate)?;
            if authority.exit_registry().is_some() {
                let prepared =
                    authority.prepare_namespace_activation(&self.state, self.view.as_ref())?;
                let proof =
                    AdmittedDiskExitPublication::from_archive(authority, &self.state, publication)?;
                self.view = Some(prepared.admit(
                    &self.state,
                    publication.original_state(),
                    &candidate.snapshot_hash,
                    &proof,
                )?);
            } else if self.view.is_some() {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "archived successor discards its namespace registry",
                ));
            }
        } else if let Some(view) = &self.view {
            let advanced = view.advance(&self.state, publication.original_state())?;
            // Re-protection reuses historical bodies. Even a fully verified
            // control archive must bind its independent payload vector to that
            // exact native backup plan; it grants no historical disk I/O.
            view.verify_backup_candidate_payloads(
                &self.state, publication.original_state(), candidate,
            )?;
            self.view = Some(advanced);
        }
        self.state = publication.visible_state();
        self.snapshot = Some(candidate.snapshot_hash.clone());
        Ok(())
    }
    fn finish(self, generation: &Generation) -> Result<Option<CurrentNamespaces>> {
        self.finish_source(&generation.base, generation.snapshot().as_deref())
    }
    pub(in crate::engine::membership_transition) fn finish_source(
        self,
        source: &State,
        snapshot: Option<&str>,
    ) -> Result<Option<CurrentNamespaces>> {
        if self.snapshot.as_deref() != snapshot
            || prepare_snapshot(&self.state, MAX_METADATA)?.hash
                != prepare_snapshot(source, MAX_METADATA)?.hash
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "namespace replay did not reach the exact frozen generation source",
            ));
        }
        Ok(self.view)
    }
}

impl AdmittedDiskExitPublication {
    fn from_archive(
        authority: &PortableAuthority,
        source: &State,
        publication: &VerifiedArchivePublication,
    ) -> Result<Self> {
        let candidate = publication.candidate();
        authority.verify_candidate(candidate)?;
        let intent = authority.intent()?;
        let boundary = intent.boundary.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "archived registry activation lacks source",
            )
        })?;
        if boundary.sequence != source.sequence
            || candidate.sequence
                != intent.fence_sequence.checked_add(1).ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimit, "activation sequence exhausted")
                })?
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "archived registry proof is not its prescribed first publication",
            ));
        }
        Ok(Self {
            admission: PublicationAdmission::Archived,
            registry: authority
                .exit_registry()
                .expect("invariant: registry activation")
                .clone(),
            occurrence_mask_hash: authority.activation_mask_hash()?,
            source_snapshot: boundary.snapshot_hash.clone(),
            source_sequence: source.sequence,
            source_business_generation: source.business_generation,
            published_snapshot: candidate.snapshot_hash.clone(),
            published_sequence: candidate.sequence,
        })
    }
}

impl Engine {
    /// Build only a read-only prefix validator. Unlike live admission this does
    /// not install a namespace, acquire a Stop gate or publish SYSTEM state.
    /// The prescribed empty-payload activation and full fixed-member barrier
    /// must precede any later candidate's local payload proof lookup.
    pub(in crate::engine) fn preflight_registry_activation(
        &self,
        candidate: &Candidate,
        original: &crate::history_package::AdmittedState,
        publication: &Publication,
        seal: &PublicationSeal,
    ) -> Result<CurrentNamespaces> {
        let generation = self.config.generation().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "registry preflight has no generation",
            )
        })?;
        generation.verify_activation_candidate(candidate)?;
        verify_publication_seal(candidate, publication, seal)?;
        generation.verify_activation_result(publication)?;
        let previous = self.replay_registry_source(generation)?;
        let prepared = generation
            .portable
            .prepare_namespace_activation(&generation.base, previous.as_ref())?;
        let boundary = generation
            .intent
            .boundary
            .as_ref()
            .expect("invariant: registry source");
        let proof = AdmittedDiskExitPublication {
            admission: PublicationAdmission::Preflight,
            registry: generation
                .registry()
                .expect("invariant: registry activation")
                .clone(),
            occurrence_mask_hash: generation.portable.activation_mask_hash()?,
            source_snapshot: boundary.snapshot_hash.clone(),
            source_sequence: boundary.sequence,
            source_business_generation: generation.base.business_generation,
            published_snapshot: candidate.snapshot_hash.clone(),
            published_sequence: candidate.sequence,
        };
        prepared.admit(
            &generation.base,
            original.state(),
            &candidate.snapshot_hash,
            &proof,
        )
    }

    pub(super) fn replay_registry_source(
        &self,
        generation: &Generation,
    ) -> Result<Option<CurrentNamespaces>> {
        let mut replay = NamespaceReplay::new();
        self.replay_generation_publications(generation, |publication| replay.advance(publication))?;
        replay.finish(generation)
    }

    /// Only the actual installed generation base can seed a new environment's
    /// receipt index. This does not install Current, admit a write, or claim
    /// that an archived publication was voted by the new member set.
    pub(in crate::engine) fn replay_generation_original_receipts(
        &self,
        generation: &Generation,
    ) -> Result<crate::engine::OriginalReceiptCursor> {
        if !self.config.generation().is_some_and(|actual| {
            std::ptr::eq(actual.as_ref(), generation)
        })
            || !Arc::ptr_eq(&self.state, &generation.base)
            || self.snapshot != generation.snapshot()
        {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix));
        }
        let mut cursor = crate::engine::OriginalReceiptCursor::default();
        let mut previous = None;
        let mut terminal = None;
        self.replay_generation_publications(generation, |publication| {
            let candidate = publication.candidate();
            if candidate.previous_snapshot != previous {
                return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix));
            }
            let visible = publication.visible_state();
            let original = publication.original_state().operations.get(&candidate.operation_id)
                .ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?;
            if cursor.needs_projection(original) || publication.publication().recovery.is_some() {
                cursor.retain_archive(
                    publication, &visible, &self.config, &self.disk_resources,
                )?;
            }
            previous = Some(candidate.snapshot_hash.clone());
            terminal = Some(visible);
            Ok(())
        })?;
        // Do not accept a scoped lookup that happened to find its one receipt.
        // Every archived slot was read; the complete terminal visible State
        // must be the same genuine base the generation installer selected.
        if previous != self.snapshot
            || !terminal.as_ref().is_some_and(|visible| visible.as_ref() == self.state.as_ref())
        {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix));
        }
        cursor.require_prefix(&self.state)?;
        Ok(cursor)
    }

    /// Both read-only consumers use the same retained archive location and
    /// verification path. No historical disk or new source locator is opened.
    pub(in crate::engine) fn replay_generation_publications(
        &self,
        generation: &Generation,
        visit: impl FnMut(&VerifiedArchivePublication) -> Result<()>,
    ) -> Result<()> {
        let disk = self
            .config
            .metadata_members()
            .find(|disk| self.disk_resources.available(disk).is_some())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::MetadataQuorumUnavailable,
                    "namespace source has no retained archive",
                )
            })?;
        let transition = object_hash(INTENT_KIND, &generation.intent)?;
        let directory =
            self.disk_resources.get(disk)?.root.private_child(
                &member_directory(disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY),
            )?;
        replay_installed_publications(
            directory,
            &generation.portable.archive_hash()?,
            &generation.portable.archive_transition()?,
            visit,
        )
    }
}

impl Generation {
    pub(in crate::engine) fn verify_activation_result(
        &self,
        publication: &Publication,
    ) -> Result<()> {
        if publication.result != self.result()? || publication.recovery.is_some() {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "registry preflight differs from its prescribed activation result",
            ));
        }
        Ok(())
    }
}
