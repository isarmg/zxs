//! Bounded replay of original publication snapshots from an installed archive.
//! Unlike namespace replay, this never overlays a recovery result or returns a
//! detached State. Consumers receive the authenticated original snapshot only.
use super::*;

pub(in crate::engine::membership_transition) fn visit(
    directory: DiskRoot,
    expected: &str,
    transition: &str,
    checkpoint: &impl Fn() -> Result<()>,
    mut visit: impl FnMut(
        &Candidate,
        &str,
        &Publication,
        &str,
        &crate::history_package::AdmittedState,
    ) -> Result<()>,
) -> Result<()> {
    const MAX_OBJECTS: usize = 32_768;
    const MAX_BYTES: usize = 128 * 1024 * 1024;
    const MAX_PUBLICATIONS: usize = 2_048;
    let store = ArchiveStore { root: directory };
    let mut bytes = 0usize;
    let mut observe = |count: usize| -> Result<()> {
        checkpoint()?;
        bytes = bytes.checked_add(count).ok_or_else(|| {
            Error::new(ErrorCode::ResourceLimit, "native archive byte budget overflow")
        })?;
        if bytes > MAX_BYTES {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "native archive exceeds 128 MiB",
            ));
        }
        Ok(())
    };
    let root: ArchiveRoot = json_codec::from_value(
        AuthenticatedMetadataObject::authenticate(
            ROOT_KIND,
            expected,
            decode_strict(&store.root.read_bytes_observed(
                &store.path(ROOT_FILE),
                MAX_METADATA,
                &mut observe,
            )?)?,
        )?
        .into_value(),
    )?;
    verify_root_copying(
        &store,
        &root,
        transition,
        None,
        &mut observe,
        Some(MAX_OBJECTS),
    )?;
    let intent: TransitionIntent =
        json_codec::from_value(store.load(INTENT_KIND, transition)?.into_value())?;
    let mut previous = None;
    let mut count = 0usize;
    for (epoch, _) in archive_epochs(&store, &root, &intent)? {
        checkpoint()?;
        let mut ordered = BTreeMap::new();
        let seals = archive_seals(&store, &epoch)?;
        if seals.len() > MAX_PUBLICATIONS - count {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "native archive exceeds 2048 publications",
            ));
        }
        count += seals.len();
        for (hash, seal_hash) in seals {
            checkpoint()?;
            let candidate: Candidate =
                json_codec::from_value(store.load("candidate", &hash)?.into_value())?;
            if ordered
                .insert(candidate.sequence, (candidate, hash, seal_hash))
                .is_some()
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "native archive repeats a sequence",
                ));
            }
        }
        for (sequence, (candidate, hash, seal_hash)) in ordered {
            checkpoint()?;
            if previous.is_some_and(|previous| sequence <= previous) {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "native archive epochs overlap",
                ));
            }
            let seal: PublicationSeal =
                json_codec::from_value(store.load(SEAL_KIND, &seal_hash)?.into_value())?;
            let publication: Publication = json_codec::from_value(
                store.load("publication", &seal.publication_hash)?.into_value(),
            )?;
            verify_publication_seal(&candidate, &publication, &seal)?;
            if publication.recovery.is_some() {
                return Err(Error::new(
                    ErrorCode::UnsupportedOperation,
                    "native move requires the original full publication prefix, not a recovery overlay",
                ));
            }
            let snapshot: Snapshot = json_codec::from_value(
                store.load("snapshot", &candidate.snapshot_hash)?.into_value(),
            )?;
            let admitted =
                resolve_admitted_snapshot(snapshot, |kind, hash| store.load(kind, hash))?;
            let state = admitted.state();
            let result = state.operations.get(&candidate.operation_id).ok_or_else(|| {
                Error::new(ErrorCode::PublicationInvalid, "native archive operation is absent")
            })?;
            if state.sequence != sequence
                || result.sequence != sequence
                || result.operation_id != candidate.operation_id
                || result.request_digest != candidate.request_digest
                || result != &publication.result
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "native archive snapshot differs from its publication",
                ));
            }
            visit(&candidate, &hash, &publication, &seal.publication_hash, &admitted)?;
            previous = Some(sequence);
        }
    }
    checkpoint()
}
