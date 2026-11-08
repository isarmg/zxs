//! Bounded no-clobber native storage. The target staging record only permits
//! continued use of the separate frozen preparation domain; it cannot select
//! a Config, registration, I/O epoch, generation marker or writer.
use super::*;
use crate::journal::AuthenticatedMetadataObject;
use std::{collections::BTreeSet, path::Path};

const TARGET_SELECTION: &str = "backup-native-metadata-selected.json";
const MAX_SELECTION: usize = 4096;

#[derive(Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
enum SelectionFormat {
    BackupUnselectedNativeMetadata,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    format: SelectionFormat,
    installation_hash: String,
    physical: PhysicalBinding,
}
fn selection(root: &DiskRoot, hash: &str) -> Result<Vec<u8>> {
    canonical_json(&Selection {
        format: SelectionFormat::BackupUnselectedNativeMetadata,
        installation_hash: hash.into(),
        physical: PhysicalBinding::of(root)?,
    })
}
pub(super) fn preflight_target(root: &DiskRoot, hash: &str) -> Result<()> {
    require_unselected_target(root)?;
    if preparation_path_exists(root, &root.path().join(TARGET_SELECTION))? {
        require_target_selection(root, hash)?;
    }
    Ok(())
}
pub(super) fn select_target(root: &DiskRoot, hash: &str) -> Result<()> {
    let bytes = selection(root, hash)?;
    install_preparation_bytes(root, &root.path().join(TARGET_SELECTION), &bytes)?;
    require_target_selection(root, hash)
}
pub(super) fn require_target_selection(root: &DiskRoot, hash: &str) -> Result<()> {
    if root.read_bytes(&root.path().join(TARGET_SELECTION), MAX_SELECTION)? != selection(root, hash)? {
        return Err(invalid("native metadata target selection changed its original plan or physical root"));
    }
    require_unselected_target(root)
}

/// No authority is obtained from this file. A selected ordinary-object stage
/// may coexist with old isolated readers because they still use only their own
/// authenticated preparation domain. Every active name remains forbidden.
pub(in super::super) fn require_unselected_target(root: &DiskRoot) -> Result<()> {
    if preparation_path_exists(root, &root.path().join("disk.json"))? {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "frozen Backup reader cannot open a sealed runtime target",
        ));
    }
    let metadata = root.path().join("metadata");
    if !preparation_path_exists(root, &metadata)? {
        return Ok(());
    }
    let bytes = root.read_bytes(&root.path().join(TARGET_SELECTION), MAX_SELECTION)?;
    let selected: Selection = crate::journal::decode_proof(&bytes)?;
    if selected.installation_hash.len() != 64
        || !selected.installation_hash.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || selected.physical != PhysicalBinding::of(root)?
        || canonical_json(&selected)? != bytes
    {
        return Err(invalid("unselected Backup metadata has no exact bounded local staging record"));
    }
    let held = root.private_child(&metadata)?;
    for entry in held.entries(held.path())? {
        let name = entry.map_err(fs_error)?;
        // In particular pool-membership/generation.json, journal namespaces,
        // I/O decisions and writer names cannot hide behind a staging record.
        if !matches!(name.to_str(), Some("objects" | "publication-seals")) {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "frozen Backup metadata contains an active or unknown namespace; retained without adoption",
            ));
        }
        held.private_child(&held.path().join(name))?;
    }
    Ok(())
}

pub(super) struct ObjectSet<'a> {
    objects: BTreeMap<(&'static str, &'a str), &'a [u8]>,
    candidate: &'a str,
    seal: &'a [u8],
    bytes: usize,
}
impl<'a> ObjectSet<'a> {
    pub(super) fn new(
        values: impl Iterator<Item = (&'static str, &'a str, &'a [u8])>,
        candidate: &'a str,
        seal: &'a [u8],
    ) -> Result<Self> {
        let mut objects = BTreeMap::new();
        let mut bytes = seal.len();
        for (kind, hash, value) in values {
            if let Some(saved) = objects.get(&(kind, hash)) {
                if *saved != value {
                    return Err(invalid("ordinary native object has colliding exact bytes"));
                }
                continue;
            }
            bytes = bytes.checked_add(value.len()).ok_or_else(budget)?;
            if objects.len() == MAX_OBJECTS || bytes > MAX_BYTES || value.len() > MAX_METADATA {
                return Err(budget());
            }
            objects.insert((kind, hash), value);
        }
        if seal.len() > MAX_METADATA || bytes > MAX_BYTES {
            return Err(budget());
        }
        Ok(Self { objects, candidate, seal, bytes })
    }
    pub(super) fn references(&self) -> Vec<ObjectReference<'_>> {
        self.objects.iter().map(|((kind, hash), bytes)| ObjectReference {
            kind, hash, bytes: bytes.len(),
        }).collect()
    }
    pub(super) fn len(&self) -> usize { self.objects.len() }
    pub(super) fn bytes(&self) -> usize { self.bytes }
    pub(super) fn get(&self, kind: &'static str, hash: &str) -> Result<&[u8]> {
        self.objects.get(&(kind, hash)).copied()
            .ok_or_else(|| invalid("native registry requested an object outside its exact closed graph"))
    }
    fn seal_path(&self, disk: &DiskConfig) -> PathBuf {
        crate::journal::publication_seal_path(disk, self.candidate)
    }
    pub(super) fn preflight(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
        reserve: u64,
        stop: &AtomicBool,
    ) -> Result<()> {
        let mut missing = 0u64;
        let mut largest = 0u64;
        for ((kind, hash), expected) in &self.objects {
            preparation_checkpoint(stop)?;
            let paths = crate::journal::existing_object_paths_at(root, disk, kind, hash)?;
            if let Some(path) = paths.first() {
                if root.read_bytes(path, expected.len())? != *expected {
                    return Err(Error::new(ErrorCode::ObjectCollision,
                        "ordinary native metadata conflicts; no existing object replaced"));
                }
            } else {
                missing = missing.checked_add(expected.len() as u64).ok_or_else(budget)?;
                largest = largest.max(expected.len() as u64);
            }
        }
        let path = self.seal_path(disk);
        if preparation_path_exists(root, &path)? {
            if root.read_bytes(&path, MAX_METADATA)? != self.seal {
                return Err(invalid("ordinary Publication seal conflicts with the real original barrier"));
            }
        } else {
            missing = missing.checked_add(self.seal.len() as u64).ok_or_else(budget)?;
            largest = largest.max(self.seal.len() as u64);
        }
        let required = missing.checked_add(largest)
            .and_then(|n| n.checked_add(reserve))
            .and_then(|n| n.checked_add(2 * MAX_DESCRIPTOR as u64 + 1024 * 1024))
            .and_then(|n| n.checked_add(self.objects.len() as u64 * 8192))
            .ok_or_else(budget)?;
        if root.available()? < required {
            return Err(Error::new(ErrorCode::NoSpace,
                "each ordinary native store needs independent bytes, atomic staging and retained reserve"));
        }
        Ok(())
    }
    pub(super) fn install(&self, root: &DiskRoot, disk: &DiskConfig, stop: &AtomicBool) -> Result<()> {
        for ((kind, hash), bytes) in &self.objects {
            preparation_checkpoint(stop)?;
            let object = AuthenticatedMetadataObject::authenticate(
                kind, hash, storage_contract::strict_json(bytes)?,
            )?.into_prepared(MAX_METADATA)?;
            if object.bytes() != *bytes {
                return Err(invalid("ordinary metadata recipe is not the original canonical native bytes"));
            }
            // Production native storage rejects flat aliases, never replaces a
            // conflicting hash name, and repeats file/directory durability.
            object.store_at_root(root, disk)?;
        }
        install_preparation_bytes(root, &self.seal_path(disk), self.seal)
    }
    pub(super) fn verify(&self, root: &DiskRoot, disk: &DiskConfig, stop: &AtomicBool) -> Result<()> {
        for ((kind, hash), bytes) in &self.objects {
            preparation_checkpoint(stop)?;
            let object = crate::journal::load_authenticated_object_at(root, disk, kind, hash)?;
            if object.bytes() != *bytes {
                return Err(invalid("ordinary native object no longer has its exact published bytes"));
            }
        }
        if root.read_bytes(&self.seal_path(disk), MAX_METADATA)? != self.seal {
            return Err(invalid("ordinary native seal lost its exact all-original member proof"));
        }
        Ok(())
    }
    pub(super) fn inventory_target(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        let metadata = root.path().join("metadata");
        if !preparation_path_exists(root, &metadata)? { return Ok(()); }
        require_unselected_target(root)?;
        let mut files = BTreeSet::new();
        for (kind, hash) in self.objects.keys() {
            files.insert(PathBuf::from("objects").join(kind).join(&hash[..2]).join(format!("{hash}.json")));
        }
        files.insert(PathBuf::from("publication-seals").join(format!("{}.json", self.candidate)));
        let mut directories = BTreeSet::new();
        let mut file_parents = BTreeSet::new();
        for path in &files {
            file_parents.insert(path.parent().expect("invariant: native object has parent").to_owned());
            let mut parent = path.parent();
            while let Some(value) = parent {
                if value == Path::new("") { break; }
                directories.insert(value.to_owned());
                parent = value.parent();
            }
        }
        let held = root.private_child(&metadata)?;
        let mut pending = vec![PathBuf::new()];
        let mut count = 0usize;
        while let Some(parent) = pending.pop() {
            for entry in held.entries(&held.path().join(&parent))? {
                preparation_checkpoint(stop)?;
                count += 1;
                if count > 4 * MAX_OBJECTS { return Err(budget()); }
                let name = entry.map_err(fs_error)?;
                let relative = parent.join(&name);
                let path = held.path().join(&relative);
                if directories.contains(&relative) {
                    held.private_child(&path)?;
                    pending.push(relative);
                } else if files.contains(&relative)
                    || (file_parents.contains(&parent)
                        && platform_io::fs_safety::AtomicFile::is_temporary_name(&name))
                {
                    held.open_read_regular(&path)?;
                } else {
                    return Err(Error::new(ErrorCode::ObjectCollision,
                        "unknown target native metadata retained, not adopted or removed"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../../../../../../test/engine/unit/backup_native_metadata_tests.rs"]
mod tests;
