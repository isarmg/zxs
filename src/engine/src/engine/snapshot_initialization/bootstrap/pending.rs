//! Read-only admission of the one original, fully FINALIZEd bootstrap decision.
//! The held journals cross the boundary intact; no cache or writer repair runs
//! until every original member and the complete initial State have been checked.
use crate::authority_selection::FreshBootstrapError;
use crate::disk::DiskResources;
use crate::engine::snapshot_initialization::bootstrap::{
    RootRecord, MAX_BYTES, decision, failure,
};
use crate::journal::{Candidate, Finalize, OpenJournal, Publication, Receipt, RecordType};
use crate::{State, ValidatedConfig};
use std::collections::BTreeMap;

pub(super) struct PendingBootstrapAdmission {
    pub(super) candidate: Candidate,
    pub(super) state: State,
    journals: BTreeMap<String, OpenJournal>,
    publication: Publication,
}

fn rejected() -> crate::Error {
    failure(FreshBootstrapError::Incomplete)
}

impl PendingBootstrapAdmission {
    pub(super) fn admit(
        config: &ValidatedConfig,
        resources: &DiskResources,
        record: &RootRecord,
    ) -> crate::Result<Self> {
        let mut members = config.metadata_members().collect::<Vec<_>>();
        members.sort_by(|left, right| left.disk_id.cmp(&right.disk_id));
        let mut journals = BTreeMap::new();
        let mut accepts = Vec::new();
        let mut finalized = Vec::new();
        let mut original: Option<(Candidate, State, Finalize)> = None;
        let mut publication_rows = Vec::new();
        for disk in members {
            let handle = resources.get(disk)?;
            config.verify_root_membership(&handle.root, disk)?;
            let opened = OpenJournal::scan_initial_at(&handle.root, disk, config, MAX_BYTES)?;
            let rows = &opened.scan.records;
            if opened.scan.torn_tail || !(3..=4).contains(&rows.len()) {
                return Err(rejected());
            }
            let candidate_hash = &rows[0].0.candidate_hash;
            for (index, (row, _)) in rows.iter().enumerate() {
                let expected = match index {
                    0 => RecordType::Prepare,
                    1 => RecordType::Accept,
                    2 => RecordType::Finalize,
                    3 => RecordType::Publication,
                    _ => return Err(rejected()),
                };
                if row.record_type != expected
                    || row.sequence != 1
                    || &row.candidate_hash != candidate_hash
                {
                    return Err(rejected());
                }
            }
            let candidate: Candidate = crate::journal::load_object_at(
                &handle.root, disk, "candidate", candidate_hash,
            )?;
            config.verify_candidate_membership(&candidate)?;
            let state = decision::read_initial_state(
                &handle.root, disk, &candidate, record,
            )?;
            let finalize: Finalize = crate::journal::load_object_at(
                &handle.root, disk, "finalize", &rows[2].0.object_hash,
            )?;
            if rows[0].0.object_hash != candidate.snapshot_hash
                || rows[1].0.object_hash != *candidate_hash
            {
                return Err(rejected());
            }
            if let Some((first, first_state, first_finalize)) = &original {
                if storage_contract::canonical_json(first)?
                    != storage_contract::canonical_json(&candidate)?
                    || first_state != &state
                    || storage_contract::canonical_json(first_finalize)?
                        != storage_contract::canonical_json(&finalize)?
                {
                    return Err(rejected());
                }
            } else {
                original = Some((candidate, state, finalize));
            }
            let receipt = |index: usize| Receipt {
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                record: rows[index].0.clone(),
                frame_hash: rows[index].1.clone(),
            };
            if rows.len() == 4 {
                publication_rows.push(((*disk).clone(), rows[3].0.object_hash.clone()));
            }
            accepts.push(receipt(1));
            finalized.push(receipt(2));
            journals.insert(disk.disk_id.clone(), opened);
        }
        let (candidate, state, finalize) = original.ok_or_else(rejected)?;
        if finalize.accepts != accepts {
            return Err(rejected());
        }
        let publication = Publication {
            candidate_hash: storage_contract::object_hash("candidate", &candidate)?,
            finalize,
            finalize_receipts: finalized,
            payload_receipts: Vec::new(),
            recovery: None,
            result: state.operations.get(&record.operation).ok_or_else(rejected)?.clone(),
        };
        let publication_hash = storage_contract::object_hash("publication", &publication)?;
        for (disk, hash) in publication_rows {
            if hash != publication_hash {
                return Err(rejected());
            }
            let handle = resources.get(&disk)?;
            let stored: Publication = crate::journal::load_object_at(
                &handle.root, &disk, "publication", &hash,
            )?;
            if storage_contract::canonical_json(&stored)?
                != storage_contract::canonical_json(&publication)?
            {
                return Err(rejected());
            }
        }
        for disk in config.metadata_members() {
            let root = &resources.get(disk)?.root;
            let path = crate::journal::publication_seal_path(disk, &publication.candidate_hash);
            if root.optional_metadata(&path)?.is_some() {
                let seal = root.read_json_observed(&path, &mut |_| Ok(()))?;
                crate::journal::verify_publication_seal(&candidate, &publication, &seal)?;
            }
        }
        let proof = decision::BootstrapDecision::from_decided(
            candidate.clone(), publication.clone(), record,
        )?;
        let bytes = storage_contract::canonical_json(&proof)?;
        // An absent known installation output is repairable. An occupied output
        // with different bytes is evidence of conflict, never overwritten.
        for disk in &config.disks {
            let root = &resources.get(disk)?.root;
            let path = disk.root.join(
                crate::persist::layout::STORAGE_INITIALIZATION_PROOF.as_path(),
            );
            if root.optional_metadata(&path)?.is_some()
                && root.read_bytes(&path, MAX_BYTES)? != bytes
            {
                return Err(rejected());
            }
        }
        Ok(Self {
            candidate,
            state,
            journals,
            publication,
        })
    }

    pub(super) fn into_parts(
        self,
    ) -> crate::Result<(
        Candidate,
        State,
        Publication,
        BTreeMap<String, crate::journal::JournalWriter>,
    )> {
        let mut writers = BTreeMap::new();
        for (disk, opened) in self.journals {
            let (writer, _) = opened.into_writer()?;
            writer.verify_retained_name()?;
            writers.insert(disk, writer);
        }
        Ok((self.candidate, self.state, self.publication, writers))
    }
}
