//! Each fixed physical root independently supplies the whole frozen archive,
//! prescribed first State, actual Publication and both native registry graphs.
use super::*;
use crate::disk::{decode_seal, identity_at, validate_stored_seal};
use crate::journal::{load_authenticated_object_at, publication_seal_path};

pub(super) struct Native {
    pub(super) first: PreparedBackupFirstCandidate,
    pub(super) publication: PreparedBackupFirstPublication,
    pub(super) registry: crate::VerifiedDiskRegistry,
    pub(super) published: AdmittedState,
    pub(super) source: AdmittedState,
    pub(super) writer: BackupWriterIoConfig,
}

pub(super) fn archive(root: &DiskRoot, portable: &PortableAuthority) -> Result<DiskRoot> {
    root.private_child(
        &root
            .path()
            .join("metadata/membership-pending")
            .join(portable.archive_transition()?)
            .join(CONTROL_ARCHIVE_DIRECTORY),
    )
}

pub(super) fn read(
    plan: &Plan,
    root: &DiskRoot,
    disk: &DiskConfig,
    completed: bool,
    stop: &AtomicBool,
) -> Result<Native> {
    checkpoint(stop)?;
    // This reads the selected authority as an actual authenticated native
    // object, before any saved plan can influence a candidate or configuration.
    let bytes = load_authenticated_object_at(root, disk, AUTHORITY_KIND, &plan.authority.hash()?)?;
    let portable: PortableAuthority = crate::journal::decode_proof(bytes.bytes())?;
    if canonical_json(&portable)? != canonical_json(&plan.authority)? || !portable.is_backup() {
        return Err(invalid(
            "Backup root lacks the exact original native authority",
        ));
    }
    let core = portable
        .backup_core()
        .ok_or_else(|| invalid("Backup root contains another lifecycle kind"))?;
    let (source, _) = portable.read_archive_base(archive(root, &portable)?)?;
    checkpoint(stop)?;
    core.verify_source(&source)?;
    let authority = PreparedMetadataObject::new(AUTHORITY_KIND, &portable, MAX_METADATA)?;
    let first = core.first_candidate(&authority, &source, stop)?;
    if first.hash() != plan.candidate {
        return Err(invalid(
            "Backup archive reconstructs another first candidate",
        ));
    }
    first.verify_on_original(root, disk, stop)?;
    let published =
        first.read_state_at(&root.private_child(&root.path().join("metadata"))?, stop)?;
    let effective = portable.effective_config()?;
    let publication_bytes =
        load_authenticated_object_at(root, disk, "publication", &plan.publication)?;
    let publication = PreparedBackupFirstPublication::from_native_bytes(
        &first,
        &effective,
        &BackupFirstPublicationHash::parse(&plan.publication)?,
        publication_bytes.bytes(),
        &root.read_bytes(&publication_seal_path(disk, first.hash()), MAX_METADATA)?,
    )?;
    let source = AdmittedState::admit_replayed(source)?;
    let source_snapshot = crate::journal::prepare_snapshot(source.state(), MAX_METADATA)?.hash;
    let expected = crate::PreparedDiskRegistry::for_backup_publication(
        first.original_config(),
        &source,
        &source_snapshot,
        &effective,
        &published,
        first.snapshot_hash(),
        first.authority_hash(),
    )?;
    if expected.commitment() != &plan.registry {
        return Err(invalid(
            "Backup native registry differs from its original common selection",
        ));
    }
    let mut objects = 0usize;
    let mut count = 0usize;
    let registry = expected.verify_backup_with(|kind, hash| {
        checkpoint(stop)?;
        let object = load_authenticated_object_at(root, disk, kind, hash)?;
        objects += 1;
        count = count.checked_add(object.bytes().len()).ok_or_else(budget)?;
        if objects > 8192 || count > 128 * 1024 * 1024 {
            return Err(budget());
        }
        Ok(object.bytes().to_vec())
    })?;
    registry.require_config(&effective)?;
    registry.require_backup_namespace(&first, &published)?;
    let writer = first
        .writer_configuration(&publication, archive(root, &portable)?, stop)?
        .bind_published_continuation(&first, &publication, registry.clone(), &published)?;
    plan.require_config(writer.config())?;
    require_selector(plan, root, disk, writer.config(), completed)?;
    if first
        .original_config()
        .metadata_members()
        .any(|member| member.disk_id == disk.disk_id)
    {
        publication.verify_original_prefix(root, disk, &first, stop, completed)?;
    }
    if !completed
        && first
            .candidate()
            .versions
            .iter()
            .any(|item| item.placement.route_slot_ids.contains(&disk.slot_id))
    {
        publication.verify_initial_ordinary_payloads(root, disk, &first, stop)?;
    }
    checkpoint(stop)?;
    Ok(Native {
        first,
        publication,
        registry,
        published,
        source,
        writer,
    })
}

pub(super) fn require_selector(
    plan: &Plan,
    root: &DiskRoot,
    disk: &DiskConfig,
    config: &ValidatedConfig,
    completed: bool,
) -> Result<()> {
    let held = root.directory(root.path())?;
    let stored = decode_seal(&root.read_bytes(&root.path().join("disk.json"), MAX_METADATA)?)?;
    let selected = identity_at(config, disk, &held)?;
    let original = plan.authority.genesis_config()?;
    let old = original
        .disks
        .iter()
        .find(|old| old.disk_id == disk.disk_id);
    // Only explicit unfinished SAME-operation recovery accepts a known old
    // source seal. A target has no old source identity and must always be new.
    let old_seal = if !completed {
        old.map(|old| identity_at(&original, old, &held))
            .transpose()?
    } else {
        None
    };
    let is_old = match old_seal {
        Some(old) => canonical_json(&stored)? == canonical_json(&old)?,
        None => false,
    };
    if !is_old {
        validate_stored_seal(&stored, &selected)?;
    }
    let selector = root
        .open_optional_read_regular(&root.path().join(GENERATION_FILE))?
        .map(|file| crate::disk::read_file_bytes(file, MAX_METADATA))
        .transpose()?;
    match selector {
        Some(bytes) if bytes == canonical_json(&plan.authority)? && !is_old => (),
        None if !completed && old.is_some() => (), // seal-first, selector-second interruption
        _ => {
            return Err(invalid(
                "Backup selector is not the exact allowed old/new same-operation state",
            ));
        }
    }
    let saved: ValidatedConfig = decode_strict(&root.read_bytes(
        &root.path().join("metadata/pool-membership/config.json"),
        MAX_METADATA,
    )?)?;
    original.verify_saved_metadata_membership(&saved)?;
    Ok(())
}

fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "Backup cold registry exceeds 8192 reads or 128 MiB",
    )
}
