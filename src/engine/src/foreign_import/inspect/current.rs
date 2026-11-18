//! The preserved record's current-pool baseline is not selected from a
//! carried Config, SQLite row or "latest" local snapshot. Require the original
//! held writer, journal frame, complete native publication and visible outcome
//! on every originally registered voter, under independent read ceilings.
use super::*;
use crate::file_stamp::FileStamp;
use crate::publication_evidence::OriginalReceiptProjection;
use std::io::BufReader;

#[derive(Debug, error_definition::Error)]
enum Changed {
    #[error("current Foreign baseline no longer matches its complete original registration")]
    Registration,
    #[error("current Foreign baseline differs from its held published journal or original State")]
    Publication,
    #[error("current Foreign journal observations exceed the original byte budget")]
    Budget,
}
impl From<Changed> for Error {
    fn from(failure: Changed) -> Self {
        let code = match failure {
            Changed::Registration => ErrorCode::DiskIdentityMismatch,
            Changed::Publication => ErrorCode::PublicationInvalid,
            Changed::Budget => ErrorCode::ResourceLimit,
        };
        storage_contract::Error::new(code, failure.to_string()).into()
    }
}

impl ForeignImportPreview {
    pub(crate) fn preserved_current_snapshot(
        &self,
        engine: &Engine,
    ) -> Result<Option<ForeignEvidenceDigest>> {
        let registered = engine.registered_disks();
        if registered.len() != engine.config.disks.len()
            || engine.journals.len() != engine.config.metadata_members().len()
        {
            return Err(Changed::Registration.into());
        }
        let state = engine.state.as_ref();
        if engine.snapshot.is_none()
            && (engine.config.generation().is_some()
                || *state != (State { topology_generation: 1, ..State::default() }))
        {
            return Err(Changed::Publication.into());
        }
        let mut metadata = Budget { remaining: self.limits.metadata_bytes };
        let mut journal_left = self.limits.journal_bytes;
        let mut fixed = None;
        let mut selected = None;
        let expected_config = object_hash("foreign-identity-current-configuration", engine.config())?;
        for registration in registered {
            let disk = engine.config.disks.iter().find(|d| d.disk_id == registration.disk_id)
                .ok_or(Changed::Registration)?;
            if registration.pool_id != engine.config.pool_id
                || registration.binding_generation != disk.binding_generation
                || registration.slot_id != disk.slot_id
                || registration.level != disk.level
                || registration.upstream_slot_id != disk.upstream_slot_id.as_deref()
                || registration.metadata_member != disk.metadata_member
                || registration.registration_mode != RegistrationMode::Enabled
            {
                return Err(Changed::Registration.into());
            }
            let held = engine.disk_resources.get(disk)?;
            let root = &held.root;
            let mut evidence = Evidence {
                root, disk: disk.clone(), budget: &mut metadata, anchors: BTreeMap::new(),
            };
            // Use the native fixed-anchor/current-generation decoder. No
            // independently implemented genesis format or substitute root.
            let local = activation::read_local_disk_configuration(root.path(), &held.seal, &mut
                |path| evidence.read(path))?;
            let exact_anchor = evidence.anchors.get(
                &root.path().join(layout::GENESIS_CONFIGURATION.as_path())
            ).ok_or(Changed::Registration)?;
            if fixed.replace(*exact_anchor).is_some_and(|prior| prior != *exact_anchor)
                || object_hash("foreign-identity-current-configuration", &local)? != expected_config
            {
                return Err(Changed::Registration.into());
            }
            if disk.metadata_member {
                let journal = engine.journals.get(&disk.disk_id).ok_or(Changed::Publication)?;
                if journal.needs_recovery() {
                    return Err(Changed::Publication.into());
                }
                journal.verify_retained_name()?;
                let last = journal_tip(root, &activation::log_path(&engine.config, disk),
                    &mut journal_left)?;
                if last.as_ref() != journal.records().last() {
                    return Err(Changed::Publication.into());
                }
                match (&engine.snapshot, last) {
                    (None, None) => {}
                    (Some(snapshot), Some((record, frame)))
                        if record.record_type == RecordType::Publication
                            && record.sequence == state.sequence => {
                        let (candidate, publication, seal) =
                            evidence.publication_and_seal(&record, &frame, &held.seal)?;
                        engine.verify_candidate_membership(&candidate)?;
                        if &candidate.snapshot_hash != snapshot
                            || selected.as_ref().is_some_and(|(prior_candidate, prior_publication)| {
                                prior_candidate != &record.candidate_hash
                                    || prior_publication != &record.object_hash
                            })
                        {
                            return Err(Changed::Publication.into());
                        }
                        let descriptor: Snapshot = evidence.object("snapshot", snapshot)?;
                        let original = resolve_admitted_snapshot(descriptor, |kind, hash| {
                            evidence.graph(kind, hash)
                        })?;
                        OriginalReceiptProjection::from_held_publication(
                            &original, &candidate, &publication, &seal, state,
                        )?;
                        selected = Some((record.candidate_hash, record.object_hash));
                    }
                    _ => return Err(Changed::Publication.into()),
                }
                journal.verify_retained_name()?;
            }
            evidence.unchanged()?;
        }
        engine.snapshot.as_deref().map(|value| value.parse().map_err(
            |source| Error::from(std::io::Error::other(source))
        )).transpose()
    }
}

fn journal_tip(
    root: &DiskRoot,
    path: &Path,
    remaining: &mut usize,
) -> Result<Option<(Record, String)>> {
    let mut file = root.open_read_regular(path)?;
    let stamp = FileStamp::from_metadata(&file.metadata()?);
    if stamp.size > *remaining as u64 {
        return Err(Changed::Budget.into());
    }
    let mut frames = JournalFrameReader::new(BufReader::new(&mut file));
    let mut last = None;
    while let Some(item) = frames.next_record_observed(&mut |count| {
        *remaining = remaining.checked_sub(count).ok_or(Changed::Budget)?;
        Ok(())
    })? {
        last = Some(item);
    }
    if frames.torn_tail || frames.valid_bytes != stamp.size {
        return Err(Changed::Publication.into());
    }
    drop(frames);
    if stamp != FileStamp::from_metadata(&file.metadata()?)
        || stamp != FileStamp::from_metadata(&root.open_read_regular(path)?.metadata()?)
    {
        return Err(Changed::Publication.into());
    }
    Ok(last)
}
