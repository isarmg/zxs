//! Actual live/detached ordinary publication reaches the same serialized
//! original-decision boundary. Freeze the physical images once on this side
//! of PREPARE and let the existing preserving alias owners consume them after
//! D3. No permanent unreferenced scope images or alternate decisions.
use super::{DiskScope, MAX_SET_BYTES, PreparedDiskScopes, prospective};
use super::super::{
    Binding, Changes, DiskHistoryHeader, InstallHistory, StoredHistory, Through, budget, invalid,
    replay,
};
use crate::{
    DiskRegistrationIdentity, Engine, OperationResult, Result, State, ValidatedConfig,
    engine::disk_catalog::{DiskCatalogReceipt, DiskCatalogSnapshot, database, snapshot},
    journal::Candidate,
};
use std::{collections::BTreeMap, sync::{Arc, atomic::AtomicBool}};
use storage_contract::{DiskId, working_tree::MaterialCommitment};

struct CurrentAlias {
    registration: DiskRegistrationIdentity,
    through: Through,
    logical_root: MaterialCommitment,
    bytes: Vec<u8>,
}
impl CurrentAlias {
    fn install(
        self,
        engine: &mut Engine,
        decided: Through,
        registration: &DiskRegistrationIdentity,
    ) -> Result<DiskCatalogReceipt> {
        if self.registration != *registration || self.through != decided {
            return Err(invalid("planned DISK current is outside its decided request"));
        }
        let current = database::read(&self.bytes, registration)?;
        let logical_root = MaterialCommitment::from_digest(
            storage_contract::canonical_object_digest("disk-current-catalog", &current)?,
        );
        if logical_root != self.logical_root
            || current != snapshot(&engine.state, registration.clone())?
        {
            return Err(invalid("planned DISK current changed before its original installation"));
        }
        engine.install_current_alias(current, logical_root.hash().to_string(), self.bytes)
    }
}

pub(in crate::engine::disk_catalog::history) struct HistoryAlias {
    registration: DiskRegistrationIdentity,
    through: Through,
    logical_root: MaterialCommitment,
    publications: u32,
    changes: usize,
    changed: bool,
    bytes: Vec<u8>,
}
impl HistoryAlias {
    pub(in crate::engine::disk_catalog::history) fn after(
        self,
        config: &ValidatedConfig,
        registration: &DiskRegistrationIdentity,
        candidate: &Candidate,
        candidate_hash: &str,
    ) -> Result<(bool, InstallHistory)> {
        let binding = Binding::published(config, candidate, candidate_hash)?;
        if self.registration != *registration || self.through != binding.through {
            return Err(invalid("planned DISK history is outside its decided request"));
        }
        let header = DiskHistoryHeader {
            identity: self.registration,
            publications: self.publications,
            through_sequence: candidate.sequence,
            through_snapshot: Some(candidate.snapshot_hash.clone()),
        };
        Ok((
            self.changed,
            InstallHistory {
                header,
                binding,
                logical_root: self.logical_root,
                changes: self.changes,
                bytes: self.bytes,
            },
        ))
    }
}

pub(in crate::engine) struct OrdinaryDiskAliases {
    sources: Source,
}
// Only the original fixed configuration currently has this efficient raw-image
// handoff. Successor membership uses its own existing archive/first-slot owner,
// never substituted genesis rows or a silently reduced registration set.
enum Source {
    Fixed {
        current: BTreeMap<DiskId, CurrentAlias>,
        history: BTreeMap<DiskId, HistoryAlias>,
        rejected: Option<(DiskRegistrationIdentity, crate::Error)>,
    },
    Successor,
}

impl OrdinaryDiskAliases {
    pub(in crate::engine) fn capture(
        engine: &Engine,
        next: &State,
        result: &OperationResult,
    ) -> Result<Self> {
        if engine.config.generation().is_some() {
            return Ok(Self { sources: Source::Successor });
        }
        let stop = AtomicBool::new(false);
        let fixed = PreparedDiskScopes::new(&engine.config)?;
        let mut rejected = None;
        let frozen = match tracked_sources(engine, &fixed) {
            Ok(Some(disks)) => prospective::freeze_from_original_rows(
                &engine.config, disks, &engine.state, next,
                engine.snapshot.as_deref(), result, &stop, encode_aliases,
            )?,
            other => {
                if let Err((identity, error)) = other {
                    // A damaged *derived alias* cannot change an original
                    // publication. Discard that input, not the file. Its own
                    // preserving installer must still check the actual prior.
                    tracing::warn!(error = ?error,
                        "unusable DISK alias; reading the fixed native publication prefix");
                    rejected = Some((identity, error));
                }
                let mut prefix = fixed;
                replay::visit_published_prefix(
                    engine, engine.state.sequence, &|| Ok(()),
                    |source| prefix.observe_published(&source, &stop),
                )?;
                prospective::freeze_from_original_rows(
                    &prefix.config, prefix.disks, &engine.state, next,
                    engine.snapshot.as_deref(), result, &stop, encode_aliases,
                )?
            }
        };
        let mut current = BTreeMap::new();
        let mut history = BTreeMap::new();
        for (a, b) in frozen {
            let id = a.registration.disk_id.parse::<DiskId>()?;
            if current.insert(id, a).is_some() || history.insert(id, b).is_some() {
                return Err(invalid("fixed DISK preparation repeats an original registration"));
            }
        }
        Ok(Self {
            sources: Source::Fixed { current, history, rejected },
        })
    }

    pub(in crate::engine) fn install_after_publication(
        self,
        engine: &mut Engine,
        previous: &Arc<State>,
        candidate: &Candidate,
        hash: &str,
    ) {
        match self.sources {
            Source::Fixed { mut current, history, rejected } => {
                engine.refresh_disk_aliases_with(Some(previous), |engine, registration| {
                    let decided = Through::from_candidate(&engine.config, candidate)?;
                    let id = registration.disk_id.parse::<DiskId>()?;
                    current.remove(&id)
                        .ok_or_else(|| invalid("decided current image is missing from its fixed set"))?
                        .install(engine, decided, registration)
                });
                engine.advance_frozen_disk_histories(previous, candidate, hash, history, rejected);
            }
            Source::Successor => {
                engine.refresh_published_disk_catalogs(Some(previous));
                engine.advance_published_disk_histories(previous, candidate, hash);
            }
        }
    }
}

// A serving-alias consumer needs the two complete physical images. Do not
// allocate a radix map only to discard it: the full immutable graph consumer
// constructs those pages from exactly the same checked rows.
fn encode_aliases(
    current: &DiskCatalogSnapshot,
    history: &StoredHistory,
) -> Result<((CurrentAlias, HistoryAlias), usize)> {
    let pair = prospective::encode_canonical_pair(current, history)?;
    let size = pair.current.len() + pair.history.len();
    let through = history.header.through;
    let a = CurrentAlias {
        registration: current.identity.clone(),
        through,
        logical_root: pair.current_root,
        bytes: pair.current,
    };
    let b = HistoryAlias {
        registration: current.identity.clone(),
        through,
        logical_root: pair.history_root,
        publications: history.header.publications,
        changes: history.changes.len(),
        changed: history.changes.last()
            .is_some_and(|row| row.sequence == current.source_sequence),
        bytes: pair.history,
    };
    Ok(((a, b), size))
}

type TrackedSources = std::result::Result<
    Option<Vec<DiskScope>>,
    (DiskRegistrationIdentity, crate::Error),
>;

fn tracked_sources(engine: &Engine, fixed: &PreparedDiskScopes) -> TrackedSources {
    if engine.state.sequence == 0 {
        return Ok(None);
    }
    let mut sources = Vec::with_capacity(fixed.disks.len());
    let mut used = 0usize;
    for disk in &fixed.disks {
        let identity = &disk.header.identity;
        let single = || -> Result<Option<DiskScope>> {
            let Some(view) = engine.tracked_history_for_next(identity)? else {
                return Ok(None);
            };
            let previous = snapshot(&engine.state, identity.clone())?;
            if super::super::offline::fold(&view, Some(&previous))? != previous {
                return Err(invalid("tracked DISK changes do not fold to their original before-state"));
            }
            let current_bytes =
                crate::disk::encode_metadata(&previous, super::MAX_RECORD_BYTES)?.len();
            let changes = Changes::retained(view.changes)?;
            let record_bytes = changes.bytes.checked_add(current_bytes).ok_or_else(budget)?;
            Ok(Some(DiskScope {
                header: view.header,
                previous,
                changes,
                record_bytes,
            }))
        };
        let Some(scope) = single().map_err(|source| (identity.clone(), source))? else {
            return Ok(None);
        };
        used = used.checked_add(scope.record_bytes)
            .filter(|value| *value <= MAX_SET_BYTES)
            .ok_or_else(|| (identity.clone(), budget()))?;
        sources.push(scope);
    }
    Ok(Some(sources))
}
