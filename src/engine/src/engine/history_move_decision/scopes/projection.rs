use super::*;

#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Scope {
    PublishedSystem {
        #[serialization(with = "storage_contract::u64_string")]
        sequence: u64,
        candidate_hash: String,
        snapshot_hash: String,
        publication_hash: String,
    },
    ProposedSystem,
    PublishedDiskCurrent {
        identity: DiskRegistrationIdentity,
    },
    PublishedDiskHistory {
        identity: DiskRegistrationIdentity,
    },
    ProposedDiskCurrent {
        identity: DiskRegistrationIdentity,
    },
}
impl Scope {
    fn name(&self) -> String {
        match self {
            Self::PublishedSystem { sequence, .. } => {
                format!("published-system-{sequence:020}.sqlite")
            }
            Self::ProposedSystem => "proposed-system.sqlite".into(),
            Self::PublishedDiskCurrent { identity } => {
                format!("published-current-{}.sqlite", identity.disk_id)
            }
            Self::PublishedDiskHistory { identity } => {
                format!("published-history-{}.sqlite", identity.disk_id)
            }
            Self::ProposedDiskCurrent { identity } => {
                format!("proposed-current-{}.sqlite", identity.disk_id)
            }
        }
    }
    fn belongs_to(&self, disk: &str) -> bool {
        match self {
            Self::PublishedSystem { .. } | Self::ProposedSystem => false,
            Self::PublishedDiskCurrent { identity }
            | Self::PublishedDiskHistory { identity }
            | Self::ProposedDiskCurrent { identity } => identity.disk_id == disk,
        }
    }
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct Descriptor {
    scope: Scope,
    name: String,
    #[serialization(with = "storage_contract::u64_string")]
    bytes: u64,
    digest: String,
}
struct Database {
    descriptor: Descriptor,
    bytes: Vec<u8>,
}
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct Manifest<'a> {
    format: &'static str,
    operation_id: OperationId,
    native_digest: &'a str,
    decision_inputs_digest: String,
    #[serialization(with = "storage_contract::u64_string")]
    published_source_sequence: u64,
    published_source_snapshot: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    unaccepted_proposed_sequence: u64,
    databases: Vec<&'a Descriptor>,
}

pub(in crate::engine::history_move_decision) struct Scopes {
    databases: Vec<Database>,
    bytes: usize,
    source_sequence: u64,
    publications: usize,
    participants: usize,
    operation: OperationId,
    native_digest: String,
    manifest: Vec<u8>,
    digest: String,
}
impl Scopes {
    pub(super) fn capture(
        engine: &Engine,
        materials: &Materials,
        native: &NativeMoveDraft,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<Self> {
        checkpoint(cancellation)?;
        let mut disks = Some(PreparedDiskScopes::new(&engine.config)?);
        let stop = AtomicBool::new(false);
        let mut scopes = Self {
            databases: Vec::new(),
            bytes: 0,
            source_sequence: engine.state.sequence,
            publications: 0,
            participants: materials.record.disks.len(),
            operation: native.operation_id(),
            native_digest: native.digest().into(),
            manifest: Vec::new(),
            digest: String::new(),
        };
        crate::engine::disk_catalog::visit_published_prefix(
            engine,
            engine.state.sequence,
            &|| checkpoint(cancellation),
            |candidate, candidate_hash, publication, publication_hash, admitted| {
                checkpoint(cancellation)?;
                // Recovery overlays and generation archives need their own exact
                // timeline recipe. Do not present original snapshots as those
                // different visible outcomes or silently start coverage later.
                if publication.recovery.is_some() {
                    return Err(Error::new(
                        ErrorCode::UnsupportedOperation,
                        "move scoped databases require the complete non-recovery genesis prefix",
                    ));
                }
                scopes.add(
                    Scope::PublishedSystem {
                        sequence: candidate.sequence,
                        candidate_hash: candidate_hash.into(),
                        snapshot_hash: candidate.snapshot_hash.clone(),
                        publication_hash: publication_hash.into(),
                    },
                    crate::engine::index::preparation::encode_observed(admitted, &|| {
                        checkpoint(cancellation)
                    })?,
                )?;
                scopes.publications += 1;
                disks
                    .as_mut()
                    .expect("invariant: prefix finishes only at its last sequence")
                    .observe(candidate, candidate_hash, admitted, &stop)?;
                if candidate.sequence == engine.state.sequence {
                    if admitted.state() != engine.state() {
                        return Err(invalid(
                            "move source timeline differs from its complete captured State",
                        ));
                    }
                    disks
                        .take()
                        .expect("invariant: nonempty source prefix has disk scopes")
                        .finish(admitted, &stop, |identity, current, history| {
                            checkpoint(cancellation)?;
                            if materials
                                .record
                                .disks
                                .iter()
                                .any(|disk| disk.identity == identity)
                            {
                                scopes.add(
                                    Scope::PublishedDiskCurrent {
                                        identity: identity.clone(),
                                    },
                                    current,
                                )?;
                                scopes.add(Scope::PublishedDiskHistory { identity }, history)?;
                            }
                            Ok(())
                        })?;
                }
                checkpoint(cancellation)
            },
        )?;
        if disks.is_some() || scopes.publications == 0 {
            return Err(invalid(
                "move scoped preparation has no complete source publication prefix",
            ));
        }
        scopes.add(
            Scope::ProposedSystem,
            crate::engine::index::preparation::encode_move_draft(native, cancellation)?,
        )?;
        for participant in &materials.record.disks {
            checkpoint(cancellation)?;
            let identity = participant.identity.clone();
            let bytes =
                crate::engine::disk_catalog::prepare_move_current(native, identity.clone())?;
            scopes.add(Scope::ProposedDiskCurrent { identity }, bytes)?;
        }
        scopes.manifest = encode_metadata(
            &Manifest {
                format: "file-history-move-unaccepted-scoped-databases",
                operation_id: scopes.operation,
                native_digest: native.digest(),
                decision_inputs_digest: materials.digest.to_string(),
                published_source_sequence: engine.state.sequence,
                published_source_snapshot: engine.snapshot.as_deref().ok_or_else(|| {
                    invalid("move scoped preparation requires the original source snapshot")
                })?,
                unaccepted_proposed_sequence: native.proposed_state().sequence,
                databases: scopes
                    .databases
                    .iter()
                    .map(|database| &database.descriptor)
                    .collect(),
            },
            MANIFEST_BYTES,
        )?;
        scopes.digest = hash(b"file-history-move-scoped-databases", &scopes.manifest)
            .to_hex()
            .to_string();
        Ok(scopes)
    }

    fn add(&mut self, scope: Scope, bytes: Vec<u8>) -> Result<()> {
        let total = self.bytes.checked_add(bytes.len()).ok_or_else(budget)?;
        if total > MAX_SET_BYTES
            || bytes.len() > MAX_DATABASE_BYTES
            || self.databases.len() == MAX_DATABASES
        {
            return Err(budget());
        }
        let descriptor = Descriptor {
            name: scope.name(),
            scope,
            bytes: bytes.len() as u64,
            digest: blake3::hash(&bytes).to_hex().to_string(),
        };
        self.bytes = total;
        self.databases.push(Database { descriptor, bytes });
        Ok(())
    }

    /// Metadata owners retain every source SYSTEM image and participant scope;
    /// a payload-only root receives only its exact disk's three local images.
    pub(in crate::engine::history_move_decision) fn artifacts<'a>(
        &'a self,
        payload_disk: Option<&str>,
    ) -> Vec<store::Artifact<'a>> {
        let mut artifacts = vec![store::Artifact::Exact {
            name: SCOPE_MANIFEST,
            bytes: &self.manifest,
        }];
        artifacts.extend(
            self.databases
                .iter()
                .filter(|database| {
                    payload_disk.is_none_or(|disk| database.descriptor.scope.belongs_to(disk))
                })
                .map(|database| store::Artifact::Scoped {
                    name: &database.descriptor.name,
                    bytes: &database.bytes,
                }),
        );
        artifacts
    }

    pub(in crate::engine::history_move_decision) fn report(
        &self,
    ) -> FileHistoryMoveScopedPreparation {
        FileHistoryMoveScopedPreparation {
            operation_id: self.operation,
            native_digest: self.native_digest.clone(),
            scoped_digest: self.digest.clone(),
            source_sequence: self.source_sequence,
            system_publications: self.publications,
            participant_disks: self.participants,
            databases: self.databases.len(),
            database_bytes: self.bytes as u64,
        }
    }
}
