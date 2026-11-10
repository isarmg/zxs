//! One storage publication path for a complete move unit. The same selected
//! candidate enters here during first execution and recovery; recovery never
//! resolves today's source path or replans the membership of the move.
use super::Engine;
use crate::{disk::*, *};
use crate::journal::{Candidate, Publication, RecordType};
use crate::publication_evidence::verified_recovery_snapshot_at;
use std::collections::{BTreeMap, BTreeSet};

mod scopes;

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::PreparationStale, message)
}
fn limit() -> Error {
    Error::new(ErrorCode::ResourceLimit, "move storage budget exhausted")
}

/// A business-validated candidate, before any storage publication. Object kind
/// and repository ownership belong to the producing business projection.
pub(crate) trait MoveOutcome {
    fn state(&self) -> &State;
    fn result(&self) -> &OperationResult;
    fn into_state(self) -> State;
}

/// The selected operation owns its original input/choice evidence. These hooks
/// bind and retire that evidence; none may publish or install a serving scope.
/// There is deliberately no alternative file/directory storage executor here.
pub(crate) trait MoveSelection<O: MoveOutcome> {
    fn verify(&self, engine: &Engine, candidate: &Candidate, hash: &str) -> Result<()>;
    fn prepare(&self, engine: &mut Engine, candidate: &Candidate, hash: &str) -> Result<()>;
    fn mark_decided(&self, engine: &mut Engine);
    fn retire(&self, engine: &mut Engine, published: &PublishedMove<O>) -> Result<()>;
    fn clear(self, engine: &mut Engine);
}

/// Only the all-member journal/seal observation below constructs this
/// capability. A bare State cannot install the three serving scopes.
pub(crate) struct PublishedMove<O = crate::business::NativeMoveCandidate> {
    pub(in crate::engine) candidate: Candidate,
    pub(in crate::engine) outcome: O,
    pub(in crate::engine) hash: String,
    files: BTreeSet<(String, String, String)>,
    retained_versions: Vec<String>,
}
impl<O: MoveOutcome> PublishedMove<O> {
    pub(crate) fn state(&self) -> &State { self.outcome.state() }
    pub(crate) fn files(&self) -> &BTreeSet<(String, String, String)> { &self.files }
}

impl Engine {
    /// Freeze once, then publish and install once. Both live execution and an
    /// admitted recovery of that exact candidate use this method. No member of
    /// an aggregate move is submitted as an independent operation.
    pub(crate) fn finish_move<O: MoveOutcome, S: MoveSelection<O>>(
        &mut self,
        candidate: Candidate,
        hash: String,
        outcome: O,
        retained_versions: Vec<String>,
        existing: Option<Publication>,
        decided: bool,
        selection: S,
    ) -> Result<OperationResult> {
        selection.verify(self, &candidate, &hash)?;
        self.require_native_move_authority()?;
        let next_namespaces = self.admitted_current_namespaces()?.map(|view| {
            let advanced = view.advance(&self.state, outcome.state())?;
            if !view.verify_history_move_candidate(&self.state, outcome.state(), &candidate)? {
                return Err(invalid("move candidate has no exact Current transition"));
            }
            Ok(std::sync::Arc::new(advanced))
        }).transpose()?;
        let files = scopes::file_scopes(self, &retained_versions, &candidate);
        scopes::require_resources(self, &candidate)?;
        scopes::verify_full(self, &retained_versions, &candidate)?;
        let mut databases = scopes::Databases::prepare(self, &candidate, &hash, decided)?;
        let transition = crate::state_projection::ProjectionTransition::new(
            &self.state, outcome.state(), None,
        )?;
        let mut names = if decided {
            crate::container_storage::prepare_recovered_names(
                &self.config, &self.disk_resources, &transition, MAX_METADATA,
            )?
        } else {
            crate::container_storage::prepare_names(
                &self.config, &self.disk_resources, &transition, MAX_METADATA,
            )?
        };
        // Empty directories have no FILE scopes. Every nonempty unit admits its
        // complete source and target set before its one journal decision.
        names.unpublished_file_database_reservations(&files)?;
        if !decided {
            names.stage_unpublished_file_databases(&mut |_| Ok(()))?;
            names.verify_unpublished_file_databases()?;
            databases.stage(self)?;
            selection.prepare(self, &candidate, &hash)?;
        }
        selection.verify(self, &candidate, &hash)?;
        scopes::require_resources(self, &candidate)?;
        // No abort/replan after this point: retain the original selector until
        // all three serving scopes and the historical roots are installed.
        self.blocked = true;
        selection.mark_decided(self);
        let finish = (|| -> Result<OperationResult> {
            if decided {
                for writer in self.journals.values_mut() {
                    writer.verify_retained_name()?;
                    writer.repair_torn_tail()?;
                    writer.verify_retained_name()?;
                }
            }
            self.complete_move_candidate(
                &candidate, &hash, outcome.state(), existing, names, decided,
            )?;
            self.observe_move_publication(&candidate, &hash, outcome.result())?;
            let published = PublishedMove {
                candidate, hash, outcome, files, retained_versions,
            };
            databases.install(self, &published)?;
            crate::container_storage::verify_move_files(
                &self.config, &self.disk_resources, &published,
            )?;
            self.install_move_system_index(&published)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint("move_after_system_database")?;
            scopes::verify_published_full(self, &published)?;
            let roots = self.verify_move_member_closure(&published)?;
            scopes::require_resources(self, &published.candidate)?;
            selection.retire(self, &published)?;
            let result = published.outcome.result().clone();
            let snapshot = published.candidate.snapshot_hash.clone();
            self.state = std::sync::Arc::new(published.outcome.into_state());
            self.current_namespaces = next_namespaces;
            self.bind_recovered_history_roots(roots);
            self.snapshot = Some(snapshot);
            self.published_state_base = None;
            selection.clear(self);
            self.blocked = false;
            Ok(result)
        })();
        finish.map_err(|error| {
            self.metadata_integrity.invalidate();
            error.context(ErrorCode::InDoubt,
                "original move may be decided; preserve both ends and resume the same operation").retryable()
        })
    }
    fn verify_move_member_closure<O: MoveOutcome>(
        &self,
        published: &PublishedMove<O>,
    ) -> Result<BTreeMap<String, String>> {
        let mut roots = None;
        for disk in self.members() {
            let members = std::slice::from_ref(&disk);
            let stored: Candidate = crate::journal::load_object_at(
                &self.disk_resources.get(&disk)?.root,
                &disk,
                "candidate",
                &published.hash,
            )?;
            self.verify_candidate_membership(&stored)?;
            let snapshot = verified_recovery_snapshot_at(
                &self.disk_resources,
                members,
                &stored.snapshot_hash,
            )?;
            let admitted = snapshot.for_candidate(&stored)?;
            if admitted.state() != published.state() {
                return Err(invalid(
                    "move member lost its exact complete candidate State",
                ));
            }
            let (_, current) = crate::history_package::bound_history_objects_with_roots_at(
                &self.disk_resources,
                members,
                &stored,
                Some(admitted),
            )?;
            if roots.as_ref().is_some_and(|original| original != &current) {
                return Err(invalid("move members disagree on complete history roots"));
            }
            roots = Some(current);
        }
        roots.ok_or_else(|| invalid("move publication has no original metadata membership"))
    }

    fn observe_move_publication(
        &self,
        candidate: &Candidate,
        hash: &str,
        result: &OperationResult,
    ) -> Result<()> {
        for disk in self.members() {
            let writer = self
                .journals
                .get(&disk.disk_id)
                .ok_or_else(|| invalid("move journal owner absent"))?;
            writer.verify_retained_name()?;
            let root = &self.disk_resources.get(&disk)?.root;
            let scan = crate::journal::scan_configured_at(root, &disk, &self.config)?;
            if scan.torn_tail {
                return Err(invalid("move publication journal has a torn tail"));
            }
            let frames: Vec<_> = scan
                .records
                .iter()
                .filter(|(record, _)| record.sequence >= candidate.sequence)
                .collect();
            if frames.len() != 4
                || frames
                    .iter()
                    .zip([
                        RecordType::Prepare,
                        RecordType::Accept,
                        RecordType::Finalize,
                        RecordType::Publication,
                    ])
                    .any(|((record, _), phase)| {
                        record.sequence != candidate.sequence
                            || record.candidate_hash != hash
                            || record.record_type != phase
                    })
                || frames[0].0.object_hash != candidate.snapshot_hash
                || frames[1].0.object_hash != hash
            {
                return Err(invalid(
                    "move does not retain every original ordered journal phase",
                ));
            }
            let receipt = frames[3];
            let publication: Publication =
                crate::journal::load_object_at(root, &disk, "publication", &receipt.0.object_hash)?;
            if publication.recovery.is_some() || &publication.result != result {
                return Err(invalid("move publication substituted its original outcome"));
            }
            if !publication.finalize.accepts.iter().any(|item| {
                item.disk_id == disk.disk_id
                    && item.record == frames[1].0
                    && item.frame_hash == frames[1].1
            }) || !publication.finalize_receipts.iter().any(|item| {
                item.disk_id == disk.disk_id
                    && item.record == frames[2].0
                    && item.frame_hash == frames[2].1
            }) {
                return Err(invalid(
                    "move publication lacks the actual original ACCEPT/FINALIZE frames",
                ));
            }
            let seal: crate::journal::PublicationSeal = root.read_json_observed(
                &crate::journal::publication_seal_path(&disk, hash),
                &mut |_| Ok(()),
            )?;
            crate::journal::verify_publication_seal(candidate, &publication, &seal)?;
            if !seal
                .receipts
                .iter()
                .any(|item| item.disk_id == disk.disk_id && item.frame_hash == receipt.1)
            {
                return Err(invalid(
                    "move seal does not retain the actual original journal frame",
                ));
            }
            writer.verify_retained_name()?;
        }
        Ok(())
    }
}
