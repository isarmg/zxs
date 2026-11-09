//! Ordinary object addresses and full per-member reads. A surviving member is
//! not a substitute for a missing dependency on another original participant.
use super::*;
use crate::journal::{existing_object_paths_at, load_authenticated_object_at};

impl FrozenCandidate {
    fn visit_objects(
        &self,
        visit: &mut impl FnMut(&'static str, &str, &[u8]) -> Result<()>,
    ) -> Result<()> {
        self.snapshot.visit_prepared_objects(visit)?;
        for object in &self.histories {
            visit(object.kind(), &object.hash, object.bytes())?;
        }
        visit(
            "candidate",
            &self.candidate_object.hash,
            self.candidate_object.bytes(),
        )
    }

    pub(super) fn describe_objects(
        &mut self,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        let mut objects = Vec::new();
        let mut bytes = 0u64;
        self.visit_objects(&mut |kind, hash, body| {
            checkpoint(cancellation)?;
            bytes = bytes
                .checked_add(body.len() as u64 + 4096)
                .ok_or_else(budget)?;
            if objects.len() == MAX_OBJECTS
                || bytes
                    .checked_add(self.system.len() as u64)
                    .ok_or_else(budget)?
                    > MAX_BYTES
            {
                return Err(budget());
            }
            objects.push(Object {
                kind,
                hash: hash.into(),
                bytes: body.len() as u64,
            });
            Ok(())
        })?;
        self.objects = objects;
        self.ordinary_bytes = bytes;
        Ok(())
    }

    pub(in crate::engine::history_move_decision) fn preflight(
        &self,
        engine: &Engine,
        materials: &Materials,
        native: &NativeMoveDraft,
        scopes: &scopes::Scopes,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        let system = store::Store::system(engine, materials, store::Mode::Read)?;
        system.require_candidate_space(self, 0)?;
        let choice_bytes = engine
            .history_move_choice
            .as_ref()
            .expect("invariant: candidate preparation requires the verified selected move")
            .storage_bytes();
        for participant in &materials.record.disks {
            checkpoint(cancellation)?;
            let disk = engine.disk(&participant.identity.slot_id)?;
            let store = store::Store::disk(engine, materials, participant, store::Mode::Read)?;
            let ordinary = if participant.carries_state() {
                self.ordinary_bytes
            } else {
                0
            };
            let mut required = store::scoped_storage_bytes(materials, participant, native, scopes)?
                .checked_add(choice_bytes)
                .and_then(|bytes| bytes.checked_add(ordinary))
                .ok_or_else(budget)?;
            for artifact in self.artifacts(participant.carries_state()) {
                required = required
                    .checked_add(artifact.bytes().len() as u64 + 4096)
                    .ok_or_else(budget)?;
            }
            if let Some(capacity) = disk.capacity_bytes
                && engine
                    .used_bytes(&disk.slot_id)
                    .checked_add(required)
                    .ok_or_else(budget)?
                    > capacity
            {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "move candidate plus all original retained material exceeds configured capacity",
                ));
            }
            let missing = if participant.carries_state() {
                self.missing_object_bytes(engine, disk, cancellation)?
            } else {
                0
            };
            store.require_candidate_space(self, missing)?;
        }
        Ok(())
    }

    fn missing_object_bytes(
        &self,
        engine: &Engine,
        disk: &DiskConfig,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<u64> {
        let root = &engine.disk_resources.get(disk)?.root;
        let mut missing = 0u64;
        self.visit_objects(&mut |kind, hash, expected| {
            checkpoint(cancellation)?;
            if existing_object_paths_at(root, disk, kind, hash)?.is_empty() {
                missing = missing
                    .checked_add(expected.len() as u64 + 4096)
                    .ok_or_else(budget)?;
            } else {
                let actual = load_authenticated_object_at(root, disk, kind, hash)?;
                if actual.bytes() != expected {
                    return Err(invalid(
                        "existing ordinary move dependency differs from its frozen source",
                    ));
                }
            }
            Ok(())
        })?;
        Ok(missing)
    }

    pub(super) fn install_objects(
        &self,
        engine: &Engine,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        for disk in engine.members() {
            checkpoint(cancellation)?;
            let member = std::slice::from_ref(&disk);
            self.snapshot.store_prepared_objects_observed_at(
                &engine.disk_resources,
                member,
                &mut || checkpoint(cancellation),
            )?;
            for object in &self.histories {
                checkpoint(cancellation)?;
                object.store_at(&engine.disk_resources, member)?;
            }
            checkpoint(cancellation)?;
            self.candidate_object
                .store_at(&engine.disk_resources, member)?;
        }
        Ok(())
    }

    pub(super) fn verify_objects(
        &self,
        engine: &Engine,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        for disk in engine.members() {
            let handle = engine.disk_resources.get(&disk)?;
            self.visit_objects(&mut |kind, hash, expected| {
                checkpoint(cancellation)?;
                let object = load_authenticated_object_at(&handle.root, &disk, kind, hash)?;
                if object.bytes() != expected {
                    return Err(invalid("ordinary move dependency differs from the independently reconstructed candidate"));
                }
                Ok(())
            })?;
            // Consume each root alone with the ordinary snapshot/history codec.
            // This admits shape/hash closure, not a journal decision or Current.
            let members = std::slice::from_ref(&disk);
            let recovered = crate::publication_evidence::verified_recovery_snapshot_at(
                &engine.disk_resources,
                members,
                &self.candidate.snapshot_hash,
            )?;
            let admitted = recovered.for_candidate(&self.candidate)?;
            if admitted.state() != self.outcome.state() {
                return Err(invalid(
                    "ordinary native snapshot does not contain the exact complete joint move",
                ));
            }
            crate::history_package::bound_history_objects_with_roots_at(
                &engine.disk_resources,
                members,
                &self.candidate,
                Some(admitted),
            )?;
            checkpoint(cancellation)?;
        }
        Ok(())
    }
}
