//! Independently verifiable data-only proof authority.
use super::*;

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
                let portable: PortableAuthority =
                    decode_strict(&read(&root.join(GENERATION_FILE))?)?;
                if portable.hash()? != *authority_hash {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "local authority differs from active seal",
                    ));
                }
                let intent = portable.intent()?;
                let Origin::Genesis { config: original } = &intent.origin;
                original.verify_saved_metadata_membership(&saved)?;
                Ok((portable, intent))
            })();
            // These files describe the shared root authority. Failed reads or
            // decode must revoke the same epoch as mismatched membership did.
            let (portable, intent) = match metadata {
                Some(metadata) => metadata.observe(authority)?,
                None => authority?,
            };
            let Origin::Genesis { config: original } = &intent.origin;
            match candidate.generation() {
                Some(_) => {
                    portable.verify_candidate(candidate)?;
                }
                None => {
                    original.verify_candidate_membership(candidate)?;
                    let hash = object_hash("candidate", candidate)?;
                    let proof: crate::metadata_map::InclusionProof = decode_strict(&read(
                        &root
                            .join("metadata/membership-history")
                            .join(format!("{hash}.json")),
                    )?)?;
                    let value = proof.verify(&portable.archive.publication_seals, &hash)?;
                    if value.as_ref().and_then(json_codec::Value::as_str).is_none()
                        || candidate.sequence >= intent.fence_sequence
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
            Ok(intent.target)
        }
    }
}
