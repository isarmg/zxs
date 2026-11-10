//! Display routing for the shared captured native move operation.
use super::*;
use crate::smb_rpc::namespace::NamespaceAdmission;
use crate::smb_rpc::object_move::{MoveContext, MoveSource, MoveTarget, RetainedMove, changed};

impl DefaultShare {
    pub(super) fn move_object(
        &mut self,
        source: SmbPathLocation,
        destination: &str,
        admission: NamespaceAdmission,
        replace: bool,
    ) -> Result<Value, ApiError> {
        let SmbRelativePath::Entry(source_path) = source.relative_path else {
            return Err(ApiError::unsupported());
        };
        let Location::Path {
            slot: target_slot,
            path: destination_path,
        } = self.locate(destination)?
        else {
            return Err(ApiError::unsupported());
        };
        let source_slot_id = source.repository.primary_slot_id.clone();
        let source_slot = self
            .shared
            .backend
            .topology_snapshot(&self.identity)?
            .slots
            .into_iter()
            .find(|slot| slot.slot_id == source_slot_id)
            .ok_or_else(changed)?;
        let source = self.pin_binding(source.repository)?;
        let (target, target_path) = match self.resolve_at(
            &target_slot,
            destination_path.clone(),
            SmbPathIntent::Lookup,
        ) {
            Ok(location) => {
                let SmbRelativePath::Entry(path) = location.relative_path else {
                    return Err(ApiError::unsupported());
                };
                let target_path =
                    name_policy::PortablePath::try_from(path).map_err(ApiError::invalid_source)?;
                let key = self.pin_binding(location.repository)?;
                (Some(key), target_path)
            }
            Err(error)
                if error.code == storage_contract::ErrorCode::DirectoryNotFound
                    && !destination_path.as_str().contains('/') =>
            {
                let target_path = name_policy::PortablePath::try_from(destination_path)
                    .map_err(ApiError::invalid_source)?;
                (None, target_path)
            }
            Err(error) => return Err(error),
        };
        self.move_context().capture(
            MoveSource {
                binding: (&source).into(),
                slot: source_slot,
                path: source_path,
            },
            MoveTarget {
                binding: target.as_ref().map(Into::into),
                slot: target_slot,
                path: target_path,
            },
            replace,
            admission,
        )
    }

    pub(super) fn resume_object_move(
        &mut self,
        source: &str,
        target: &str,
        replace: bool,
        operation: OperationId,
    ) -> Result<Option<Value>, ApiError> {
        let Some(retained) = RetainedMove::read(&self.move_context(), operation)? else {
            return Ok(None);
        };
        let intent = &retained.intent;
        let Location::Path {
            slot: source_slot,
            path: source_path,
        } = self.locate(source)?
        else {
            return Err(changed());
        };
        let Location::Path {
            slot: target_slot,
            path: target_path,
        } = self.locate(target)?
        else {
            return Err(changed());
        };
        if intent.replace_requested != replace
            || source_slot.slot_id != intent.source_slot_id
            || target_slot.slot_id != intent.movement.target_slot_id
            || !intent.source_disk.matches(&source_slot)
            || !intent.target_disk.matches(&target_slot)
        {
            return Err(changed());
        }
        // Resolve only the displayed repository binding. A completed move can
        // remove the source occurrence; it must never capture new members here.
        let source = self.resolve_at(&source_slot, source_path, SmbPathIntent::Lookup)?;
        let SmbRelativePath::Entry(source_path) = source.relative_path else {
            return Err(changed());
        };
        if !intent
            .source_binding
            .matches(&(&self.pin_binding(source.repository)?).into())
            || source_path.key() != intent.movement.source_path.key()
        {
            return Err(changed());
        }
        match &intent.target_binding {
            Some(binding) => {
                let target = self.resolve_at(&target_slot, target_path, SmbPathIntent::Lookup)?;
                let SmbRelativePath::Entry(path) = target.relative_path else {
                    return Err(changed());
                };
                if !binding.matches(&(&self.pin_binding(target.repository)?).into())
                    || path.as_str() != intent.movement.target_path.as_str()
                {
                    return Err(changed());
                }
            }
            // An originally empty disk may now own the published root. Keep
            // the captured None intent rather than resolving it as a new target.
            None if target_path.as_str() != intent.movement.target_path.as_str() => {
                return Err(changed());
            }
            None => {}
        }
        retained.submit(&self.move_context()).map(Some)
    }

    fn move_context(&self) -> MoveContext<'_> {
        MoveContext {
            shared: &self.shared,
            identity: &self.identity,
            account_generation: self.account_generation,
        }
    }
}
