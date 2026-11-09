//! Original ordinary object databases inside the one Backup switch. This is an
//! internal storage adapter, not a separately selectable publication stage.
//! The coordinator owns native admission, all-member selection and completion.
use super::{backup_databases, directory_database, file_database};
use crate::{
    Error, ErrorCode, Result, canonical_json,
    database_install::{InstallAdmission, PreparedInstall},
    disk::{DiskRoot, FileStamp, HeldRegular},
    history_package::{AdmittedState, preparation_checkpoint},
};
use data_serialization::Serialize;
use platform_io::fs_safety::linux::FileIdentity;
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool};

mod entity;
mod selection;
use entity::Entity;

const MAX_RECORD: usize = 4 * 1024 * 1024;
const COMPLETION_RESERVE: u64 = 2 * MAX_RECORD as u64 + 1024 * 1024;

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "original Backup object database set exceeds its bounded metadata budget",
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
struct Physical {
    device: u64,
    inode: u64,
}
impl From<FileIdentity> for Physical {
    fn from(value: FileIdentity) -> Self {
        Self {
            device: value.device,
            inode: value.inode,
        }
    }
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
enum Key {
    File(String, String),
    Directory(String, String),
}
impl Key {
    fn of(scope: &backup_databases::Scope) -> Self {
        match scope {
            backup_databases::Scope::File { history, file } => {
                Self::File(history.clone(), file.clone())
            }
            backup_databases::Scope::Directory {
                history, directory, ..
            } => Self::Directory(history.clone(), directory.clone()),
        }
    }
}

struct Object {
    entity: Entity,
    bytes: u64,
    digest: String,
}
impl Object {
    fn read_published(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<HeldRegular> {
        self.entity.check(root, stop)?;
        let before = crate::file_stamp::FileStamp::from_metadata(
            &self
                .entity
                .root
                .optional_metadata(&self.entity.root.path().join(self.entity.leaf()))?
                .ok_or_else(|| invalid("completed original object database is missing"))?,
        );
        let mut actual =
            HeldRegular::capture(&self.entity.root, self.entity.leaf(), self.entity.maximum())?
                .ok_or_else(|| invalid("completed original object database is missing"))?;
        if actual.stamp().bytes != self.bytes || actual.stamp().digest != self.digest {
            return Err(invalid(
                "original object database differs from its exact published State image",
            ));
        }
        // The expected digest comes from freshly encoded and strictly verified
        // native State, never from the installed database's own records_hash.
        actual.check_running()?;
        self.entity.check(root, stop)?;
        let metadata = self
            .entity
            .root
            .optional_metadata(&self.entity.root.path().join(self.entity.leaf()))?
            .ok_or_else(|| invalid("original object database disappeared during readback"))?;
        if crate::file_stamp::FileStamp::from_metadata(&metadata) != before
            || Physical::from(FileIdentity::from_metadata(&metadata))
                != (Physical {
                    device: actual.stamp().device,
                    inode: actual.stamp().inode,
                })
        {
            return Err(invalid("original object database changed during readback"));
        }
        actual.release_bytes();
        Ok(actual)
    }
}

struct Evidence {
    root: DiskRoot,
    physical: FileIdentity,
    objects: Vec<Object>,
    recipe_hash: String,
}
impl Evidence {
    fn check_root(&self) -> Result<()> {
        if self.root.directory_identity(self.root.path())? != self.physical
            || DiskRoot::open(self.root.path())?.directory_identity(self.root.path())?
                != self.physical
        {
            return Err(invalid(
                "original Backup object root no longer names its held identity",
            ));
        }
        Ok(())
    }
    fn verify(&self, stop: &AtomicBool) -> Result<Completion> {
        preparation_checkpoint(stop)?;
        self.check_root()?;
        let mut installed = Vec::with_capacity(self.objects.len());
        for object in &self.objects {
            preparation_checkpoint(stop)?;
            installed.push(object.read_published(&self.root, stop)?);
        }
        // Recheck the complete set, not just the last installed object. The
        // writer lease/fence is retained by the outer native transaction.
        for (object, expected) in self.objects.iter().zip(&installed) {
            if object.read_published(&self.root, stop)?.stamp() != expected.stamp() {
                return Err(invalid(
                    "original object database set changed during readback",
                ));
            }
        }
        self.check_root()?;
        #[derive(Serialize)]
        struct Record<'a> {
            format: &'static str,
            recipe_hash: &'a str,
            installed: Vec<&'a FileStamp>,
        }
        let bytes = canonical_json(&Record {
            format: "zxs-backup-original-object-completion",
            recipe_hash: &self.recipe_hash,
            installed: installed.iter().map(HeldRegular::stamp).collect(),
        })?;
        if bytes.len() > MAX_RECORD {
            return Err(budget());
        }
        Ok(Completion { bytes })
    }
}

/// Only a fresh read of every actual ordinary database can create this proof.
/// The coordinator persists bytes and compares later fresh proofs exactly; no
/// decoder can turn a completion record into permission to install or repair.
pub(crate) struct Completion {
    bytes: Vec<u8>,
}
impl Completion {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub(crate) struct Plan {
    evidence: Evidence,
    installs: Vec<PreparedInstall>,
}
impl Plan {
    pub(crate) fn capture(
        root: &DiskRoot,
        slot: &str,
        original: &AdmittedState,
        published: &AdmittedState,
        stop: &AtomicBool,
    ) -> Result<Self> {
        name_policy::validate_slot(slot)?;
        preparation_checkpoint(stop)?;
        let physical = root.directory_identity(root.path())?;
        let held = root.private_child_matching(root.path(), physical)?;
        let mut selected = selection::capture(&held, slot, original.state(), stop)?;
        let mut objects = Vec::with_capacity(selected.len());
        let mut installs = Vec::with_capacity(selected.len());
        for database in
            backup_databases::PreparedDatabases::capture(published, stop)?.into_databases()
        {
            preparation_checkpoint(stop)?;
            let (scope, bytes) = database.into_parts();
            let Some(entity) = selected.remove(&Key::of(&scope)) else {
                continue;
            };
            if entity.scope != scope {
                return Err(invalid(
                    "published object scope differs from its original ordinary owner",
                ));
            }
            entity.check(&held, stop)?;
            let object = Object {
                entity,
                bytes: bytes.len() as u64,
                digest: blake3::hash(&bytes).to_hex().to_string(),
            };
            let install = PreparedInstall::capture(
                &held,
                &object.entity.history,
                (object.entity.root.path(), object.entity.inode),
                (
                    object.entity.install_scope()?,
                    InstallAdmission::DecidedRecovery,
                ),
                bytes,
                object.entity.maximum(),
                |old| object.entity.verify_scope(old),
            )?;
            installs.push(install);
            objects.push(object);
        }
        if !selected.is_empty() {
            return Err(invalid(
                "published State omits an original routed FILE or initialized directory database",
            ));
        }
        #[derive(Serialize)]
        struct Recipe<'a> {
            format: &'static str,
            slot: &'a str,
            root: Physical,
            objects: Vec<entity::Descriptor<'a>>,
        }
        let bytes = canonical_json(&Recipe {
            format: "zxs-backup-original-object-recipe",
            slot,
            root: physical.into(),
            objects: objects
                .iter()
                .map(|object| object.entity.descriptor(object.bytes, &object.digest))
                .collect(),
        })?;
        if bytes.len() > MAX_RECORD {
            return Err(budget());
        }
        let evidence = Evidence {
            root: held,
            physical,
            objects,
            recipe_hash: blake3::hash(&bytes).to_hex().to_string(),
        };
        evidence.check_root()?;
        Ok(Self { evidence, installs })
    }

    pub(crate) fn recipe_hash(&self) -> &str {
        &self.evidence.recipe_hash
    }

    /// Call for every original participant before publishing any selection.
    /// Existing decided tasks reuse their retained stages, never delete them.
    pub(crate) fn require_capacity(
        &self,
        admission: InstallAdmission,
        reserve: u64,
        stop: &AtomicBool,
    ) -> Result<()> {
        preparation_checkpoint(stop)?;
        self.evidence.check_root()?;
        let mut histories = BTreeMap::<&PathBuf, (u64, usize)>::new();
        let mut total = COMPLETION_RESERVE.checked_add(reserve).ok_or_else(budget)?;
        for (object, install) in self.evidence.objects.iter().zip(&self.installs) {
            preparation_checkpoint(stop)?;
            object.entity.check(&self.evidence.root, stop)?;
            let (bytes, entries) = install.reservation();
            let accumulated = histories.entry(&object.entity.history).or_default();
            accumulated.0 = accumulated.0.checked_add(bytes).ok_or_else(budget)?;
            accumulated.1 = accumulated.1.checked_add(entries).ok_or_else(budget)?;
            total = total.checked_add(bytes).ok_or_else(budget)?;
        }
        for (history, (bytes, entries)) in histories {
            admission.check(&self.evidence.root, history, bytes, entries)?;
        }
        if self.evidence.root.available()? < total {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "original Backup databases must retain their complete installation and metadata reserve",
            ));
        }
        Ok(())
    }

    /// Only after the coordinator has durably selected every original root.
    /// No selector/seal/locator/body is written here; every old database inode
    /// remains in the existing preserving install envelope through recovery.
    pub(crate) fn apply_preserving(self, stop: &AtomicBool) -> Result<Completion> {
        let Self { evidence, installs } = self;
        preparation_checkpoint(stop)?;
        evidence.check_root()?;
        for (object, install) in evidence.objects.iter().zip(installs) {
            preparation_checkpoint(stop)?;
            object.entity.check(&evidence.root, stop)?;
            install.apply(&evidence.root, &mut |_| preparation_checkpoint(stop))?;
            object.read_published(&evidence.root, stop)?;
        }
        evidence.verify(stop)
    }

    /// Completion observation never calls an installer, creates missing paths,
    /// or uses another disk/isolated target image as a fallback for this root.
    pub(crate) fn verify(&self, stop: &AtomicBool) -> Result<Completion> {
        self.evidence.verify(stop)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/backup_original_objects_tests.rs"]
mod tests;
