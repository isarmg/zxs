//! Independently verifiable data-only proof authority.
use super::*;

fn read_generation_configuration(
    root: &Path,
    seal: &DiskSeal,
    authority_hash: &str,
    saved: &ValidatedConfig,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>>,
) -> Result<(PortableAuthority, ValidatedConfig)> {
    let portable: PortableAuthority = decode_strict(&read(&root.join(GENERATION_FILE))?)?;
    if portable.hash()? != authority_hash {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "local authority differs from active seal",
        ));
    }
    let effective = portable.effective_config()?;
    portable
        .genesis_config()?
        .verify_saved_metadata_membership(saved)?;
    if let Some(dependency) = portable.data_dependency(&seal.disk_id)? {
        dependency.verify_active_seal(seal)?;
        dependency.verify_bytes(&read(&dependency.path(root))?)?;
    }
    Ok((portable, effective))
}

/// Read this root's recorded topology without admitting a Candidate, metadata
/// quorum, current namespace or foreign-pool import. The caller retains every
/// file read beneath its explicitly held root and verifies the physical seal.
pub(crate) fn read_local_disk_configuration(
    root: &Path,
    seal: &DiskSeal,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>>,
) -> Result<ValidatedConfig> {
    let saved = ValidatedConfig::try_from(decode_strict::<Config>(&read(
        &root.join("metadata/pool-membership/config.json"),
    )?)?)?;
    let config = match &seal.format {
        DiskSealFormat::FreshDirectorySnapshot { .. } => {
            crate::authority_selection::FreshSelection::admit_disk(saved, seal, &read(
                &root.join(crate::authority_selection::FRESH_DISK_SELECTOR),
            )?)?
        }
        DiskSealFormat::ManagedDirectory => saved,
        DiskSealFormat::MembershipGeneration { authority_hash } => {
            read_generation_configuration(root, seal, authority_hash, &saved, read)?.1
        }
    };
    if config.pool_id != seal.pool_id
        || (config.mode == Mode::Development) != seal.simulation
        || !config.disks.iter().any(|disk| {
            disk.disk_id == seal.disk_id
                && disk.slot_id == seal.slot_id
                && disk.binding_generation == seal.binding_generation
        })
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "local configuration does not bind this disk seal",
        ));
    }
    Ok(config)
}

/// Local proofs keep their original Candidate bytes. The root seal chooses a
/// generation; the candidate selects either its new authority or one exact
/// archived hash, never an arbitrary sequence below the frozen frontier.
pub(crate) fn verify_local_membership(
    root: &Path,
    candidate: &Candidate,
    seal: &DiskSeal,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>>,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<ValidatedConfig> {
    let saved = (|| {
        ValidatedConfig::try_from(decode_strict::<Config>(&read(
            &root.join("metadata/pool-membership/config.json"),
        )?)?)
    })();
    match &seal.format {
        DiskSealFormat::FreshDirectorySnapshot { .. } => {
            let admitted = (|| {
                let config = crate::authority_selection::FreshSelection::admit_disk(
                    saved?, seal,
                    &read(&root.join(crate::authority_selection::FRESH_DISK_SELECTOR))?,
                )?;
                config.verify_candidate_membership(candidate)?;
                Ok(config)
            })();
            match metadata {
                Some(metadata) => metadata.membership(admitted),
                None => admitted,
            }
        }
        DiskSealFormat::ManagedDirectory => {
            let saved = match metadata {
                Some(metadata) => metadata.membership(saved)?,
                None => saved?,
            };
            saved.verify_candidate_membership(candidate)?;
            Ok(saved)
        }
        DiskSealFormat::MembershipGeneration { authority_hash } => {
            let authority = (|| {
                let saved = saved?;
                read_generation_configuration(root, seal, authority_hash, &saved, read)
            })();
            // These files describe the shared root authority. Failed reads or
            // decode must revoke the same epoch as mismatched membership did.
            let (portable, effective) = match metadata {
                Some(metadata) => metadata.observe(authority)?,
                None => authority?,
            };
            match candidate.generation() {
                Some(hash) if hash == authority_hash => {
                    portable.verify_candidate(candidate)?;
                }
                _ => {
                    let (archive, fence_sequence) = portable.frozen_archive(candidate)?;
                    let hash = object_hash("candidate", candidate)?;
                    let proof: crate::metadata_map::InclusionProof = decode_strict(&read(
                        &root
                            .join("metadata/membership-history")
                            .join(format!("{hash}.json")),
                    )?)?;
                    let value = proof.verify(&archive.publication_seals, &hash)?;
                    if value.as_ref().and_then(json_codec::Value::as_str).is_none()
                        || candidate.sequence >= fence_sequence
                    {
                        return Err(Error::new(
                            ErrorCode::PublicationInvalid,
                            "local candidate is outside exact frozen archive",
                        ));
                    }
                }
            }
            if let Some(metadata) = metadata {
                metadata.generation_candidate(authority_hash, candidate)?;
            }
            Ok(effective)
        }
    }
}
