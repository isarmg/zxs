use super::{codec, model::Object, *};
use data_serialization::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum Availability {
    Available,
    TemporarilyUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CopyRow {
    pub owner: Identity,
    pub history_id: Token,
    pub file_id: Token,
    pub commit: Digest,
    pub body: Digest,
    pub availability: Availability,
}

#[derive(Clone, Debug)]
pub struct VerifiedDisk {
    pub(crate) identity: Identity,
    pub(crate) local_binding: Option<super::storage::LocalBinding>,
    pub(crate) checkpoint: Option<Digest>,
    pub(crate) sequence: u64,
    pub(crate) rows: Vec<CopyRow>,
    pub(crate) bundle: Bundle,
    pub(crate) publications: BTreeMap<Digest, Publication>,
}
impl VerifiedDisk {
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn checkpoint(&self) -> Option<Digest> {
        self.checkpoint
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn copies(&self) -> &[CopyRow] {
        &self.rows
    }
    pub fn portable_bundle(&self) -> Bundle {
        self.bundle.clone()
    }
    pub fn source_checkpoint(&self) -> Result<SourceCheckpoint> {
        Ok(SourceCheckpoint {
            identity: self.identity.clone(),
            publication: self
                .checkpoint
                .ok_or(Error::Invalid("empty disk has no published checkpoint"))?,
        })
    }
    pub fn receipt(&self, operation: &Token) -> Option<DiskPublicationReceipt> {
        self.publications
            .iter()
            .find(|(_, p)| p.identity == self.identity && &p.operation_id == operation)
            .map(|(hash, p)| p.receipt(*hash))
    }
}

pub(crate) trait ReadStore {
    fn object(&self, hash: Digest) -> Result<Vec<u8>>;
    fn body(&self, hash: Digest) -> Result<Option<Vec<u8>>>;
}

pub(crate) fn verify(
    store: &impl ReadStore,
    identity: &Identity,
    checkpoint: Digest,
) -> Result<VerifiedDisk> {
    identity.validate()?;
    let mut walker = Walker {
        store,
        bundle: Bundle::default(),
        active: BTreeSet::new(),
        publications: BTreeMap::new(),
        rows: BTreeMap::new(),
        bytes: 0,
    };
    let publication = walker.publication(checkpoint, identity, 0)?;
    Ok(VerifiedDisk {
        identity: identity.clone(),
        local_binding: None,
        checkpoint: Some(checkpoint),
        sequence: publication.sequence,
        rows: walker.rows.into_values().collect(),
        bundle: walker.bundle,
        publications: walker.publications,
    })
}

struct Walker<'a, S> {
    store: &'a S,
    bundle: Bundle,
    active: BTreeSet<Digest>,
    publications: BTreeMap<Digest, Publication>,
    rows: BTreeMap<(Origin, Digest), CopyRow>,
    bytes: usize,
}
impl<S: ReadStore> Walker<'_, S> {
    fn object(&mut self, hash: Digest) -> Result<Object> {
        if let Some(bytes) = self.bundle.objects.get(&hash) {
            return codec::decode(bytes);
        }
        if self.bundle.objects.len() >= MAX_OBJECTS {
            return Err(Error::Budget);
        }
        let bytes = self.store.object(hash)?;
        self.bytes = self.bytes.checked_add(bytes.len()).ok_or(Error::Budget)?;
        if self.bytes > MAX_BYTES {
            return Err(Error::Budget);
        }
        if codec::address("object", &bytes) != hash {
            return Err(Error::Corrupt(hash.to_string()));
        }
        let object: Object = codec::decode(&bytes)?;
        self.bundle.objects.insert(hash, bytes);
        Ok(object)
    }
    fn publication(
        &mut self,
        hash: Digest,
        identity: &Identity,
        depth: usize,
    ) -> Result<Publication> {
        if depth > MAX_PUBLICATIONS {
            return Err(Error::Budget);
        }
        if let Some(known) = self.publications.get(&hash) {
            if &known.identity != identity {
                return Err(Error::Identity);
            }
            return Ok(known.clone());
        }
        if !self.active.insert(hash) {
            return Err(Error::Invalid("cyclic publication proof"));
        }
        let Object::Publication(publication) = self.object(hash)? else {
            return Err(Error::Invalid("source is not a publication decision"));
        };
        if &publication.identity != identity {
            return Err(Error::Identity);
        }
        identity.validate()?;
        let Object::Intent(intent) = self.object(publication.intent)? else {
            return Err(Error::Invalid("publication has no intent"));
        };
        let Object::Prepared(prepared) = self.object(publication.prepared)? else {
            return Err(Error::Invalid("publication has no prepared proof"));
        };
        let Object::Manifest(manifest) = self.object(publication.manifest)? else {
            return Err(Error::Invalid("publication has no manifest"));
        };
        if intent.target != *identity || manifest.identity != *identity {
            return Err(Error::Identity);
        }
        if intent.operation_id != publication.operation_id
            || intent.base != publication.previous
            || intent.manifest != publication.manifest
            || intent.request_digest != publication.request_digest
            || prepared.intent != publication.intent
            || prepared.manifest != publication.manifest
            || prepared.request_digest != publication.request_digest
            || prepared.required_bodies != intent.required_bodies
            || intent.request_digest != request_digest(&intent)?
            || manifest.sequence != publication.sequence
            || manifest.authority.role() != identity.role
        {
            return Err(Error::Invalid("publication proof fields disagree"));
        }
        if let Some(previous) = publication.previous {
            let prior = self.publication(previous, identity, depth + 1)?;
            if prior.sequence.checked_add(1) != Some(publication.sequence) {
                return Err(Error::Invalid("publication sequence is not contiguous"));
            }
            if self
                .publications
                .values()
                .any(|p| p.identity == *identity && p.operation_id == publication.operation_id)
            {
                return Err(Error::OperationConflict);
            }
        } else if publication.sequence != 1 {
            return Err(Error::Invalid("invalid first publication sequence"));
        }
        match &manifest.authority {
            Authority::Primary { heads } => {
                require_unique(heads.iter().copied())?;
                for head in heads {
                    self.commit(*head, identity, None, 0)?;
                }
            }
            Authority::Secondary { sources } | Authority::Tertiary { sources } => {
                if sources.is_empty() {
                    return Err(Error::Invalid(
                        "backup requires an explicitly selected source",
                    ));
                }
                let expected = if identity.role == Role::Secondary {
                    Role::Primary
                } else {
                    Role::Secondary
                };
                let mut identities = BTreeSet::new();
                for source in sources {
                    if source.identity.role != expected
                        || source.identity.origin.disk_id == identity.origin.disk_id
                    {
                        return Err(Error::Invalid(
                            "wrong-level or self-referential backup source",
                        ));
                    }
                    if !identities.insert(source.identity.origin.clone()) {
                        return Err(Error::Invalid("duplicate backup source identity"));
                    }
                    self.publication(source.publication, &source.identity, depth + 1)?;
                }
            }
        }
        for (copy, required) in &intent.required_bodies {
            let Object::Commit(commit) = self.object(*copy)? else {
                return Err(Error::Invalid(
                    "required FULL copy does not identify a commit",
                ));
            };
            if commit.body != *required || !self.rows.contains_key(&(commit.owner.origin, *copy)) {
                return Err(Error::Invalid(
                    "required FULL copy differs from the reachable version",
                ));
            }
        }
        if self
            .publications
            .values()
            .any(|known| known.identity.origin == identity.origin && known.identity != *identity)
        {
            return Err(Error::Identity);
        }
        if self.publications.iter().any(|(known_hash, known)| {
            known.identity == *identity
                && known.sequence == publication.sequence
                && *known_hash != hash
        }) {
            return Err(Error::Invalid(
                "conflicting publication forks for one disk identity",
            ));
        }
        self.active.remove(&hash);
        self.publications.insert(hash, publication.clone());
        Ok(publication)
    }
    fn commit(
        &mut self,
        hash: Digest,
        identity: &Identity,
        parent_of: Option<(&Token, &Token)>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_PUBLICATIONS {
            return Err(Error::Budget);
        }
        let Object::Commit(commit) = self.object(hash)? else {
            return Err(Error::Invalid("history head is not a commit"));
        };
        if commit.owner != *identity || identity.role != Role::Primary {
            return Err(Error::Identity);
        }
        if parent_of
            .is_some_and(|(history, file)| *history != commit.history_id || *file != commit.file_id)
        {
            return Err(Error::Invalid(
                "parent belongs to a different history or file",
            ));
        }
        if self.rows.contains_key(&(identity.origin.clone(), hash)) {
            return Ok(());
        }
        if !self.active.insert(hash) {
            return Err(Error::Invalid("cyclic commit graph"));
        }
        require_unique(commit.parents.iter().copied())?;
        for parent in &commit.parents {
            self.commit(
                *parent,
                identity,
                Some((&commit.history_id, &commit.file_id)),
                depth + 1,
            )?;
        }
        let bytes = if let Some(known) = self.bundle.bodies.get(&hash) {
            Some(known.clone())
        } else {
            self.store.body(hash)?
        };
        let availability = if let Some(bytes) = bytes {
            if bytes.len() as u64 != commit.body.bytes || Digest::body(&bytes) != commit.body.digest
            {
                return Err(Error::Corrupt(hash.to_string()));
            }
            if !self.bundle.bodies.contains_key(&hash) {
                self.bytes = self.bytes.checked_add(bytes.len()).ok_or(Error::Budget)?;
                if self.bytes > MAX_BYTES {
                    return Err(Error::Budget);
                }
                self.bundle.bodies.insert(hash, bytes);
            }
            Availability::Available
        } else {
            Availability::TemporarilyUnavailable
        };
        self.rows.insert(
            (identity.origin.clone(), hash),
            CopyRow {
                owner: identity.clone(),
                history_id: commit.history_id,
                file_id: commit.file_id,
                commit: hash,
                body: commit.body.digest,
                availability,
            },
        );
        self.active.remove(&hash);
        Ok(())
    }
}

fn require_unique(values: impl Iterator<Item = Digest>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(Error::Invalid("duplicate graph reference"));
        }
    }
    Ok(())
}

pub(crate) fn request_digest(intent: &PublicationIntent) -> Result<Digest> {
    Ok(codec::address(
        "request",
        &codec::encode(&(
            intent.format,
            &intent.operation_id,
            &intent.target,
            intent.base,
            intent.manifest,
            intent.execution_binding,
            &intent.required_bodies,
        ))?,
    ))
}
