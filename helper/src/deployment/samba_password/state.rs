//! Observation of an existing local tdbsam target.
pub(super) mod enrollment;
use crate::{
    Result,
    account::Snapshot,
    system_program::{Executable, FileStamp},
};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    RelativePath,
    linux::{AdvisoryLock, FileIdentity, MountPolicy, OpenAt2Root, SingleLinkRequirement},
};
use std::{fs::File, os::unix::fs::MetadataExt, path::Path};

#[derive(Debug)]
enum ObservationError {
    ParameterText(std::str::Utf8Error),
    ExistingAccountText(std::str::Utf8Error),
    ExistingAccountUid(std::num::ParseIntError),
    NativeAccountsText(std::str::Utf8Error),
    NativeAccountUid(std::num::ParseIntError),
}
impl std::fmt::Display for ObservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::ParameterText(_) => "Samba parameter reply is not UTF-8",
            Self::ExistingAccountText(_) => "Samba account query returned invalid text",
            Self::ExistingAccountUid(_) => "Samba account UID is malformed",
            Self::NativeAccountsText(_) => "native Samba account listing is not UTF-8",
            Self::NativeAccountUid(_) => "native Samba account UID is malformed",
        };
        f.write_str(message)
    }
}
impl std::error::Error for ObservationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ParameterText(source)
            | Self::ExistingAccountText(source)
            | Self::NativeAccountsText(source) => Some(source),
            Self::ExistingAccountUid(source) | Self::NativeAccountUid(source) => Some(source),
        }
    }
}

pub(super) const PRIVATE: &str = "/var/lib/samba/private";
pub(super) const DATABASE: &str = "/var/lib/samba/private/passdb.tdb";
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Revision {
    parent_device: String,
    parent_inode: String,
    file: FileStamp,
}
struct Parent {
    path: &'static str,
    root: OpenAt2Root,
}
impl Parent {
    fn checked(path: &'static str, root: OpenAt2Root) -> Result<Self> {
        check_parent(root.directory(), path == PRIVATE)?;
        Ok(Self { path, root })
    }
    fn child(&self, name: &str, path: &'static str) -> Result<Self> {
        Self::checked(
            path,
            OpenAt2Root::from_directory(
                self.root.open_directory(&RelativePath::new(name)?)?,
                MountPolicy::AllowNestedMounts,
            )?,
        )
    }
    fn recheck(&self) -> Result<()> {
        let now = OpenAt2Root::open(Path::new(self.path), MountPolicy::AllowNestedMounts)?;
        check_parent(now.directory(), self.path == PRIVATE)?;
        if now.identity() != self.root.identity() {
            return Err(
                "Samba database parent identity changed; no replacement is admitted".into(),
            );
        }
        Ok(())
    }
}
pub(super) struct Database {
    parents: Vec<Parent>,
    file: File,
    identity: FileIdentity,
    original: Revision,
    _lock: AdvisoryLock,
}
impl Database {
    pub(super) fn open() -> Result<Self> {
        let parents = parents()?;
        let private = parents
            .last()
            .expect("invariant: fixed Samba private parent");
        let lock = AdvisoryLock::directory(private.root.directory())?;
        let file = private
            .root
            .open_file(&RelativePath::new("passdb.tdb")?, false)?;
        let identity = check_database(&file)?;
        let original = Revision {
            parent_device: private.root.identity().device.to_string(),
            parent_inode: private.root.identity().inode.to_string(),
            file: FileStamp::of(&file.metadata()?),
        };
        let result = Self {
            parents,
            file,
            identity,
            original,
            _lock: lock,
        };
        result.require_original()?;
        Ok(result)
    }
    fn private(&self) -> &Parent {
        self.parents
            .last()
            .expect("invariant: database retains its fixed private parent")
    }
    pub(super) fn revision(&self) -> Result<Revision> {
        for parent in &self.parents {
            parent.recheck()?;
        }
        let named = self
            .private()
            .root
            .open_file(&RelativePath::new("passdb.tdb")?, false)?;
        if check_database(&named)? != self.identity || check_database(&self.file)? != self.identity
        {
            return Err("Samba database name no longer identifies the admitted file; preserve the uncertain operation".into());
        }
        Ok(Revision {
            parent_device: self.private().root.identity().device.to_string(),
            parent_inode: self.private().root.identity().inode.to_string(),
            file: FileStamp::of(&self.file.metadata()?),
        })
    }
    pub(super) fn require_original(&self) -> Result<()> {
        if self.revision()? != self.original {
            return Err("existing Samba database changed since the preview; password command was not admitted".into());
        }
        Ok(())
    }
    pub(super) fn synchronize(&self) -> Result<Revision> {
        self.revision()?;
        self.file.sync_all()?;
        self.private().root.directory().sync_all()?;
        self.revision()
    }
}
fn parents() -> Result<Vec<Parent>> {
    let var = Parent::checked(
        "/var",
        OpenAt2Root::open(Path::new("/var"), MountPolicy::AllowNestedMounts)?,
    )?;
    let lib = var.child("lib", "/var/lib")?;
    let samba = lib.child("samba", "/var/lib/samba")?;
    let private = samba.child("private", PRIVATE)?;
    Ok(vec![var, lib, samba, private])
}
fn check_parent(file: &File, private: bool) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_dir()
        || metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
        || (private && metadata.mode() & 0o7777 != 0o700)
    {
        return Err("existing Samba parents must be root-owned and not group/other writable; the private directory must be mode 0700; nothing was repaired".into());
    }
    Ok(())
}
fn check_database(file: &File) -> Result<FileIdentity> {
    let identity = SingleLinkRequirement::check(file)?;
    let metadata = file.metadata()?;
    if metadata.uid() != 0 || metadata.mode() & 0o7777 != 0o600 || metadata.len() == 0 {
        return Err("Samba passdb must already be a nonempty root-owned single-link mode-0600 file; no database is created".into());
    }
    Ok(identity)
}
pub(super) fn query(program: &Executable, parameter: &str) -> Result<String> {
    let bytes = program.capture(&[
        "--suppress-prompt",
        "--debuglevel=0",
        "--parameter-name",
        parameter,
        crate::private_io::SAMBA_PATH,
    ])?;
    scalar(&bytes)
}
fn scalar(bytes: &[u8]) -> Result<String> {
    let value = std::str::from_utf8(bytes)
        .map_err(ObservationError::ParameterText)?
        .trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err("Samba parameter reply is missing or is not a single bounded scalar".into());
    }
    Ok(value.into())
}
pub(super) fn require_local_configuration(program: &Executable) -> Result<()> {
    if query(program, "server role")? != "standalone server"
        || !matches!(
            query(program, "passdb backend")?.as_str(),
            "tdbsam" | "tdbsam:/var/lib/samba/private/passdb.tdb"
        )
        || query(program, "private dir")? != PRIVATE
        || !query(program, "unix password sync")?.eq_ignore_ascii_case("no")
        || !query(program, "pam password change")?.eq_ignore_ascii_case("no")
    {
        return Err("only the exact standalone local tdbsam backend at /var/lib/samba/private/passdb.tdb, without Unix/PAM password synchronization, is supported; remote or custom backends are not changed".into());
    }
    Ok(())
}
pub(super) fn require_existing_account(program: &Executable, account: &Snapshot) -> Result<()> {
    let bytes = program.capture(&[
        "--debuglevel=0",
        "--configfile",
        crate::private_io::SAMBA_PATH,
        "--list",
        "--user",
        account.name.as_str(),
    ])?;
    account_listing(&bytes, account.name.as_str(), account.uid)
}
fn account_listing(bytes: &[u8], name: &str, uid: u32) -> Result<()> {
    let text = std::str::from_utf8(bytes).map_err(ObservationError::ExistingAccountText)?;
    let mut lines = text.lines();
    let line = lines
        .next()
        .ok_or("Samba account does not exist; reset cannot add it")?;
    if lines.next().is_some() {
        return Err("Samba account query did not return exactly one record".into());
    }
    let mut fields = line.splitn(3, ':');
    let current_name = fields.next().ok_or("Samba account reply is malformed")?;
    let current_uid = fields
        .next()
        .ok_or("Samba account reply is malformed")?
        .parse::<u32>()
        .map_err(ObservationError::ExistingAccountUid)?;
    if current_name != name || current_uid != uid || fields.next().is_none() {
        return Err(
            "existing Samba account differs from the selected local non-root account".into(),
        );
    }
    Ok(())
}
#[cfg(test)]
#[path = "../../../../test/helper/unit/samba_password_state.rs"]
mod tests;
