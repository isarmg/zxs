//! Held, service-owned empty layout and the normal setup writer lock. All file
//! publication uses admitted directory descriptors, never a reopened substitute.
use crate::offline::{
    Result,
    account::{LocalAccount, Name, Snapshot},
    private_io::BundleDestination,
};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    AdvisoryLock, AtomicFile, EntryName, Error, PrivateDirectory, RelativePath,
    linux::{FileIdentity, MountPolicy, OpenAt2Root, open_private_regular},
};
use std::{fs::File, io::Read, os::unix::fs::MetadataExt, path::Path};
use storage_server::{
    auth::{AuthStore, PreparedInitialAdministrator},
    management::MANAGEMENT_AUTHORITY_BYTES,
    setup_recovery::management_initialization_record,
};

const DATA: &str = "/zxs/data";
const STATE: &str = "/zxs/data/state";
const MANAGEMENT: &str = "/zxs/data/management";
const DISKS: &str = "/zxs/data/disks";
const INTENT: &str = "management-initialization.json";
const AUTHORITY: &str = "management-authority.json";
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Identity {
    device: String,
    inode: String,
}
impl From<FileIdentity> for Identity {
    fn from(value: FileIdentity) -> Self {
        Self {
            device: value.device.to_string(),
            inode: value.inode.to_string(),
        }
    }
}
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Context {
    host: Identity,
    data: Identity,
    state: Identity,
    management: Identity,
    disks: Identity,
    service_account: Snapshot,
}
pub(super) struct Target {
    pub context: Context,
    account: LocalAccount,
    data: OpenAt2Root,
    state: OpenAt2Root,
    management: OpenAt2Root,
    disks: OpenAt2Root,
    state_private: PrivateDirectory,
    _setup: AdvisoryLock,
    // The lifecycle lease is dropped only after setup and all private handles.
    pub host: BundleDestination,
}
impl Target {
    pub fn open(service_user: String) -> Result<Self> {
        if rustix::process::geteuid().as_raw() != 0 {
            return Err(
                "first administrator preparation requires the manually run root helper".into(),
            );
        }
        let account = LocalAccount::resolve(Name::new(service_user)?)?;
        let host = BundleDestination::open(Path::new("/zxs"))?;
        let data = OpenAt2Root::open(Path::new(DATA), MountPolicy::AllowNestedMounts)?;
        check_owner(data.directory(), account.owner())?;
        let state = child(&data, "state", account.owner())?;
        let management = child(&data, "management", account.owner())?;
        let disks = child(&data, "disks", account.owner())?;
        let state_private = administrative(&state, STATE)?;
        let setup = AdvisoryLock::acquire(&state_private, &RelativePath::new("setup.lock")?)?;
        let context = Context {
            host: host.identity().into(),
            data: data.identity().into(),
            state: state.identity().into(),
            management: management.identity().into(),
            disks: disks.identity().into(),
            service_account: account.snapshot(),
        };
        let target = Self {
            context,
            account,
            data,
            state,
            management,
            disks,
            state_private,
            _setup: setup,
            host,
        };
        target.recheck()?;
        Ok(target)
    }
    pub fn recheck(&self) -> Result<()> {
        self.host.recheck()?;
        self.account.recheck()?;
        for (path, held) in [
            (DATA, &self.data),
            (STATE, &self.state),
            (MANAGEMENT, &self.management),
            (DISKS, &self.disks),
        ] {
            let current = OpenAt2Root::open(Path::new(path), MountPolicy::AllowNestedMounts)?;
            check_owner(current.directory(), self.account.owner())?;
            if current.identity() != held.identity() {
                return Err("first administrator layout changed identity; preserve original operation evidence".into());
            }
        }
        check_entries(&self.data, &["state", "management", "disks"])?;
        check_entries(&self.disks, &[])?;
        self.state_private.verify_identity()?;
        Ok(())
    }
    pub fn admit_empty(&self) -> Result<()> {
        self.recheck()?;
        check_entries(&self.state, &["setup.lock"])?;
        check_entries(&self.management, &["auth.lock"])?;
        verify_optional(&self.state_private, "setup.lock", b"")?.ok_or("setup lock disappeared")?;
        let management = self.management_directory()?;
        let _ = verify_optional(&management, "auth.lock", b"")?;
        Ok(())
    }
    pub fn admit_original(&self, candidate: &PreparedInitialAdministrator) -> Result<()> {
        self.recheck()?;
        check_entries(&self.state, &["setup.lock", INTENT, AUTHORITY])?;
        check_entries(&self.management, &["auth.lock", "management-auth.json"])?;
        verify_optional(&self.state_private, "setup.lock", b"")?.ok_or("setup lock disappeared")?;
        let intent = verify_optional(
            &self.state_private,
            INTENT,
            &management_initialization_record(),
        )?
        .is_some();
        let authority =
            verify_optional(&self.state_private, AUTHORITY, MANAGEMENT_AUTHORITY_BYTES)?.is_some();
        let management = self.management_directory()?;
        let _ = verify_optional(&management, "auth.lock", b"")?;
        let auth_exists = match management
            .read_bounded(&EntryName::new("management-auth.json")?, 4 * 1024 * 1024)
        {
            Ok(_) => true,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if (auth_exists && !intent) || (authority && !auth_exists) {
            return Err("initial management evidence violates the original durable order; no missing authority was guessed or recreated".into());
        }
        if auth_exists {
            let auth = AuthStore::open_system_management_directory(management)?;
            if auth.credential_revision()? != candidate.credential_revision()? {
                return Err("existing administrator or sessions differ from the original initialization; no credential reset is allowed here".into());
            }
        }
        Ok(())
    }
    pub fn management_directory(&self) -> Result<PrivateDirectory> {
        administrative(&self.management, MANAGEMENT)
    }
    pub fn publish_intent(&self) -> Result<()> {
        self.recheck()?;
        publish_marker(
            &self.state_private,
            INTENT,
            &management_initialization_record(),
        )?;
        self.recheck()
    }
    pub fn publish_authority(&self) -> Result<()> {
        self.recheck()?;
        publish_marker(&self.state_private, AUTHORITY, MANAGEMENT_AUTHORITY_BYTES)?;
        self.recheck()
    }
    pub fn verify_complete(&self) -> Result<()> {
        self.recheck()?;
        verify_optional(
            &self.state_private,
            INTENT,
            &management_initialization_record(),
        )?
        .ok_or("management intent disappeared")?;
        verify_optional(&self.state_private, AUTHORITY, MANAGEMENT_AUTHORITY_BYTES)?
            .ok_or("management authority marker disappeared")?;
        Ok(())
    }
}
fn administrative(root: &OpenAt2Root, path: &str) -> Result<PrivateDirectory> {
    let directory = PrivateDirectory::from_directory_for_administration(
        root.directory().try_clone()?,
        path.into(),
    )?;
    directory.verify_identity()?;
    Ok(directory)
}
fn child(parent: &OpenAt2Root, name: &str, owner: (u32, u32)) -> Result<OpenAt2Root> {
    let file = parent.open_directory(&EntryName::new(name)?.as_relative())?;
    check_owner(&file, owner)?;
    Ok(OpenAt2Root::from_directory(file, MountPolicy::SameMount)?)
}
fn check_owner(file: &File, owner: (u32, u32)) -> Result<()> {
    let metadata = file.metadata()?;
    if (metadata.uid(), metadata.gid()) != owner || metadata.mode() & 0o7777 != 0o700 {
        return Err("first administrator requires the exact selected UID/GID and 0700 private layout; existing permissions were not changed".into());
    }
    Ok(())
}
fn check_entries(directory: &OpenAt2Root, allowed: &[&str]) -> Result<()> {
    for name in directory.entries()? {
        let name = name?;
        if !allowed
            .iter()
            .any(|allowed| name.as_os_str() == std::ffi::OsStr::new(allowed))
        {
            return Err("unexpected existing installation material; first administrator cannot adopt, delete or initialize it".into());
        }
    }
    Ok(())
}
fn verify_optional(
    directory: &PrivateDirectory,
    name: &str,
    expected: &[u8],
) -> Result<Option<File>> {
    let name = EntryName::new(name)?;
    let file = match open_private_regular(directory, &name) {
        Ok(file) => file,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() != expected.len() as u64 {
        return Err(
            "initial management control record has an unexpected size; it was preserved".into(),
        );
    }
    let mut bytes = Vec::new();
    (&file)
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes != expected {
        return Err("initial management control record differs; it was not replaced".into());
    }
    directory.verify_payload(&name, &file)?;
    Ok(Some(file))
}
fn publish_marker(directory: &PrivateDirectory, name: &str, expected: &[u8]) -> Result<()> {
    directory.verify_identity()?;
    if verify_optional(directory, name, expected)?.is_none() {
        AtomicFile::stage(directory, &EntryName::new(name)?)?
            .prepare(expected)?
            .create()?;
    }
    let file = verify_optional(directory, name, expected)?
        .ok_or("initial management control publication disappeared")?;
    file.sync_all()?;
    directory.sync()?;
    directory.verify_payload(&EntryName::new(name)?, &file)?;
    directory.verify_identity()?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/server/unit/offline/initialization/administrator_target_tests.rs"]
mod tests;
