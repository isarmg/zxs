//! Reconstruct inherited namespace admission from exact frozen publications,
//! never from a raw current head or a serialized view supplied by a directory.
use super::*;
use crate::engine::disk_registry::current::CurrentNamespaces;
use crate::engine::membership_transition::archive::persistent::namespace_replay::{
    VerifiedArchivePublication, replay_installed_publications,
};

struct NamespaceReplay {
    state: Arc<State>,
    snapshot: Option<String>,
    view: Option<CurrentNamespaces>,
}
impl NamespaceReplay {
    fn new() -> Self {
        Self {
            state: Arc::new(State {
                topology_generation: 1,
                ..State::default()
            }),
            snapshot: None,
            view: None,
        }
    }
    fn advance(&mut self, publication: &VerifiedArchivePublication) -> Result<()> {
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
                    publication.state(),
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
            self.view = Some(view.advance(&self.state, publication.state())?);
        }
        self.state = Arc::clone(publication.state());
        self.snapshot = Some(candidate.snapshot_hash.clone());
        Ok(())
    }
    fn finish(self, generation: &Generation) -> Result<Option<CurrentNamespaces>> {
        if self.snapshot != generation.snapshot()
            || prepare_snapshot(&self.state, MAX_METADATA)?.hash
                != prepare_snapshot(&generation.base, MAX_METADATA)?.hash
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
    pub(super) fn replay_registry_source(
        &self,
        generation: &Generation,
    ) -> Result<Option<CurrentNamespaces>> {
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
        let mut replay = NamespaceReplay::new();
        replay_installed_publications(
            directory,
            &generation.portable.archive_hash()?,
            &transition,
            |publication| replay.advance(publication),
        )?;
        replay.finish(generation)
    }
}
