//! The only fresh Add layout writer. Saved feedback observes exact stage/final
//! identities; an existing but unrecorded stage is never a creation outcome.
use crate::{Config, DiskConfig, Engine, Result, ValidatedConfig};
use crate::disk::{DirectoryCreateOutcome, DiskRoot, MAX_METADATA, encode_metadata, fs_error};
use crate::engine::fresh_layout::{self, FreshLayoutError as Failure};
use crate::engine::fresh_layout::root_areas::HeldRootAreas;
use crate::engine::membership_transition::pending::admission;
use crate::engine::membership_transition::pending::reservation::{
    ReservedRoot, lock, name, verify_marker, verify_writer,
};
use crate::engine::membership_transition::pending::reservation::fresh_addition::{
    Preparation, PreparationFormat, RecordedAdditionArea, origin,
};
use crate::persist::layout;
use crate::working_tree::records::{file_identity, local_entity};
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::{collections::BTreeMap, sync::Arc};
use storage_contract::{DiskId, Generation, OperationId, Principal};
use storage_contract::working_tree::{
    DirectoryId, LocalDirectoryBinding, RootBinding, RootId, StorageLayout,
};

pub(super) struct HeldAddition {
    root: Arc<DiskRoot>,
    physical: FileIdentity,
    writer: Arc<crate::disk::FenceOwner>,
}
impl HeldAddition {
    pub(super) fn lock(
        engine: &Engine, root: DiskRoot, disk: &DiskConfig, operation: OperationId,
        saved: Option<&Preparation>, others: &BTreeMap<OperationId, Preparation>,
    ) -> Result<Self> {
        let directory = root.directory(root.path())?;
        lock(&directory)?;
        let physical = admission::verify_named_root(&root)?;
        if engine.disk_resources.handles().any(|old| {
            old.seal.device == physical.device && old.seal.inode == physical.inode
        }) || DiskRoot::open(&engine.root)?.directory_identity(&engine.root)? == physical {
            return Err(Failure::AdditionEvidence.into());
        }
        if others.values().any(|other| file_identity(other.root.root_entity) == physical)
            || engine.read_source_preparations()?.iter().any(|(op, previous)| {
                *op != operation && previous.added_roots.values().any(|root| {
                    root.device == physical.device && root.inode == physical.inode
                })
            })
        {
            return Err(Failure::AdditionConflict.into());
        }
        if let Some(saved) = saved {
            if disk.disk_id.parse::<DiskId>()? != saved.disk_id
                || physical != file_identity(saved.root.root_entity)
            {
                return Err(Failure::AdditionEvidence.into());
            }
            let target = saved.target(&engine.root)?;
            let identity = admission::member_identity(&target, disk, physical);
            let reservation = name(&saved.origin, &target, saved.principal.as_str(),
                saved.operation_id, &identity)?;
            inspect_names(&root, Some((
                &layout::membership_layout_stage(saved.stage.directory_id), &reservation,
            )))?;
        } else {
            inspect_names(&root, None)?;
        }
        let writer = root.lock_file()?;
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&writer).map_err(fs_error)?;
        lock(&writer)?;
        if writer.metadata()?.len() != 0 {
            return Err(Failure::AdditionEvidence.into());
        }
        writer.sync_all()?;
        directory.sync_all()?;
        let held = Self {
            root: Arc::new(root), physical,
            writer: crate::disk::FenceOwner::new(writer, Some(directory)),
        };
        held.recheck()?;
        Ok(held)
    }

    fn recheck(&self) -> Result<()> {
        if admission::verify_named_root(&self.root)? != self.physical {
            return Err(Failure::AdditionEvidence.into());
        }
        verify_writer(&self.root, &self.writer)
    }

    pub(super) fn create(
        &self, engine: &mut Engine, request: Config, principal: &Principal,
        operation: OperationId, disk: &DiskConfig,
    ) -> Result<(Preparation, HeldRootAreas)> {
        // All identifiers precede the first mkdir; no observed name/inode is an ID source.
        let root_id = RootId::generate()?;
        let stage_id = DirectoryId::generate()?;
        let ids = [DirectoryId::generate()?, DirectoryId::generate()?, DirectoryId::generate()?,
            DirectoryId::generate()?, DirectoryId::generate()?];
        self.recheck()?;
        inspect_names(&self.root, None)?;
        let name = layout::membership_layout_stage(stage_id);
        let (stage, entity) = match self.root.create_private_entry(&name) {
            DirectoryCreateOutcome::Created { directory, identity, durability } => {
                durability.map_err(fresh_layout::directory_error)?;
                self.root.sync_private_entry_matching(&name, identity)
                    .map_err(fresh_layout::directory_error)?;
                (directory, local_entity(identity))
            }
            DirectoryCreateOutcome::Rejected(error)
            | DirectoryCreateOutcome::Uncertain { source: error, .. } => {
                return Err(fresh_layout::directory_error(error));
            }
        };
        let areas = HeldRootAreas::create(&stage, ids)?;
        self.recheck()?;
        areas.verify(&stage)?;
        #[cfg(feature = "fault-injection")]
        engine.failpoint("after_membership_layout_unrecorded_stage")?;
        let registration = fresh_layout::registration(&request, disk);
        let source_chain = fresh_layout::source_chain(&request, disk)?;
        let value = Preparation {
            format: PreparationFormat::FreshRoot, origin: origin(&engine.config), request,
            principal: principal.clone(),
            operation_id: operation,
            disk_id: disk.disk_id.parse()?,
            stage: LocalDirectoryBinding { directory_id: stage_id, entity },
            root: RootBinding {
                layout: StorageLayout::UnifiedWorkingTreeV1, root_id,
                registration, source_chain,
                root_entity: local_entity(self.physical), roots: areas.binding().clone(),
                operation_id: operation, generation: Generation::new(1),
            },
        };
        // The completed record, not a partial marker, is bounded before it is
        // synchronized on the original fixed members.
        encode_metadata(&value, MAX_METADATA)?;
        Ok((value, areas))
    }

    pub(super) fn observe(&self, value: &Preparation) -> Result<HeldRootAreas> {
        self.recheck()?;
        let stage = self.stage(value)?;
        let areas = HeldRootAreas::observe_installation(&self.root, stage.as_ref(),
            &value.root.roots)?;
        areas.require_empty()?;
        Ok(areas)
    }

    fn stage(&self, value: &Preparation) -> Result<Option<DiskRoot>> {
        let name = layout::membership_layout_stage(value.stage.directory_id);
        if self.root.optional_metadata(&self.root.path().join(name.as_path()))?.is_none() {
            return Ok(None);
        }
        let stage = self.root.private_entry_matching(&name, file_identity(value.stage.entity))
            .map_err(fresh_layout::directory_error)?;
        for name in stage.entries(stage.path())? {
            let name = name.map_err(fs_error)?;
            if name != *layout::COMMON_DATA_ROOT && name != *layout::DELETED_DATA_ROOT
                && name != *layout::ZXS_DATA_ROOT
            {
                return Err(Failure::AdditionEvidence.into());
            }
        }
        Ok(Some(stage))
    }

    pub(super) fn install(
        self, engine: &mut Engine, value: &Preparation, target: &ValidatedConfig,
        areas: HeldRootAreas,
    ) -> Result<ReservedRoot> {
        self.observe(value)?;
        let stage = self.stage(value)?;
        for (index, (name, expected)) in [
            (&*layout::COMMON_DATA_ROOT, &value.root.roots.common_data),
            (&*layout::DELETED_DATA_ROOT, &value.root.roots.deleted_data),
            (&*layout::ZXS_DATA_ROOT, &value.root.roots.zxs_data),
        ].into_iter().enumerate() {
            self.recheck()?;
            let destination = self.root.path().join(name.as_path());
            if self.root.optional_metadata(&destination)?.is_none() {
                let stage = stage.as_ref().ok_or(Failure::AdditionEvidence)?;
                DiskRoot::install_recorded_addition_area(&RecordedAdditionArea {
                    root: &self.root, stage, writer: &self.writer,
                    preparation: value, name, expected,
                })?;
            }
            self.root.sync_private_entry_matching(name, file_identity(expected.entity))
                .map_err(fresh_layout::directory_error)?;
            #[cfg(feature = "fault-injection")]
            engine.failpoint(&format!("after_membership_layout_area_install_{}", index + 1))?;
            #[cfg(not(feature = "fault-injection"))]
            let _ = index;
        }
        areas.verify(&self.root)?;
        areas.require_empty()?;
        self.recheck()?;
        let disk = target.disks.iter().find(|disk| disk.disk_id == value.disk_id.to_string())
            .ok_or(Failure::AdditionEvidence)?;
        fresh_layout::verify_root(target, disk, &self.root)?;
        let identity = admission::member_identity(target, disk, self.physical);
        let reservation = name(&value.origin, target, value.principal.as_str(),
            value.operation_id, &identity)?;
        let directory = self.root.directory(self.root.path())?;
        match rustix::fs::mkdirat(&directory, reservation.as_str(),
            rustix::fs::Mode::from_raw_mode(0o700))
        {
            Ok(()) | Err(rustix::io::Errno::EXIST) => (),
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
        verify_marker(&self.root, &reservation)?;
        self.root.sync_dir(&self.root.path().join(&reservation))?;
        directory.sync_all()?;
        if let Some(stage) = stage {
            self.root.remove_owned_empty_directory_synced(stage.path(),
                file_identity(value.stage.entity))?;
        }
        let reserved = ReservedRoot {
            root: self.root, identity, name: reservation, writer: self.writer,
            layout: Some(target.clone()),
        };
        reserved.verify_unsealed()?;
        Ok(reserved)
    }
}

fn inspect_names(root: &DiskRoot, recorded: Option<(&EntryName, &str)>) -> Result<()> {
    for entry in root.entries(root.path())? {
        let entry = entry.map_err(fs_error)?;
        if entry == *layout::DISK_WRITER_LOCK {
            if root.open_regular_entry(&layout::DISK_WRITER_LOCK)?.metadata()?.len() != 0 {
                return Err(Failure::AdditionEvidence.into());
            }
            continue;
        }
        let Some((stage, reservation)) = recorded else {
            return Err(Failure::AdditionUnrecorded.into());
        };
        if entry == *stage || entry.as_os_str() == reservation
            || entry == *layout::COMMON_DATA_ROOT || entry == *layout::DELETED_DATA_ROOT
            || entry == *layout::ZXS_DATA_ROOT
        {
            continue;
        }
        return Err(Failure::AdditionEvidence.into());
    }
    Ok(())
}
