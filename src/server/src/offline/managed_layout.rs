//! Fixed identities shared by the root coordinator and the service-UID worker.
//! No permission repair, mount operation, path substitution or account creation.
use crate::offline::{
    Result,
    account::{LocalAccount, Name, Snapshot},
};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    EntryName, PrivateDirectory,
    linux::{FileIdentity, MountPolicy, OpenAt2Root},
};
use std::{
    collections::BTreeSet,
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

pub(crate) const STATE: &str = "/zxs/data/state";
pub(crate) const MANAGEMENT: &str = "/zxs/data/management";
pub(crate) const DISKS: &str = "/zxs/data/disks";
const DATA: &str = "/zxs/data";
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Context {
    host: Identity,
    data: Identity,
    state: Identity,
    management: Identity,
    discovery: Identity,
    pub account: Snapshot,
}
pub(crate) struct Layout {
    account: LocalAccount,
    data: OpenAt2Root,
    state: OpenAt2Root,
    management: OpenAt2Root,
    disks: OpenAt2Root,
}
impl Layout {
    pub fn open(user: String) -> Result<Self> {
        let account = LocalAccount::resolve(Name::new(user)?)?;
        let data = OpenAt2Root::open(Path::new(DATA), MountPolicy::AllowNestedMounts)?;
        owner(data.directory(), account.owner())?;
        let child = |name: &str| -> Result<OpenAt2Root> {
            let file = data.open_directory(&EntryName::new(name)?.as_relative())?;
            owner(&file, account.owner())?;
            Ok(OpenAt2Root::from_directory(
                file,
                if name == "disks" {
                    MountPolicy::AllowNestedMounts
                } else {
                    MountPolicy::SameMount
                },
            )?)
        };
        let layout = Self {
            state: child("state")?,
            management: child("management")?,
            disks: child("disks")?,
            account,
            data,
        };
        layout.recheck()?;
        Ok(layout)
    }
    pub fn context(&self, host: FileIdentity) -> Context {
        Context {
            host: host.into(),
            data: self.data.identity().into(),
            state: self.state.identity().into(),
            management: self.management.identity().into(),
            discovery: self.disks.identity().into(),
            account: self.account.snapshot(),
        }
    }
    pub fn recheck(&self) -> Result<()> {
        self.account.recheck()?;
        for (path, held) in [
            (DATA, &self.data),
            (STATE, &self.state),
            (MANAGEMENT, &self.management),
            (DISKS, &self.disks),
        ] {
            let named = OpenAt2Root::open(Path::new(path), MountPolicy::AllowNestedMounts)?;
            owner(named.directory(), self.account.owner())?;
            if named.identity() != held.identity() {
                return Err(
                    "managed directory identity changed; original material was preserved".into(),
                );
            }
        }
        entries(&self.data, &["state", "management", "disks"])
    }
    pub fn control(&self) -> Result<()> {
        let state = private(&self.state, STATE)?;
        if state.read_bounded(&EntryName::new("management-initialization.json")?, 4096)?
            != storage_server::setup_recovery::management_initialization_record()
            || state.read_bounded(&EntryName::new("management-authority.json")?, 4096)?
                != storage_server::management::MANAGEMENT_AUTHORITY_BYTES
        {
            return Err("first storage requires the current completed management authority".into());
        }
        Ok(())
    }
    pub fn fresh(&self) -> Result<()> {
        self.control()?;
        entries(
            &self.state,
            &[
                "setup.lock",
                "management-initialization.json",
                "management-authority.json",
            ],
        )?;
        entries(&self.management, &["auth.lock", "management-auth.json"])
    }
    pub fn state(&self) -> Result<PrivateDirectory> {
        private(&self.state, STATE)
    }
    pub fn management(&self) -> Result<PrivateDirectory> {
        private(&self.management, MANAGEMENT)
    }

    /// Directory creation is limited to management-only first installation.
    /// It cannot add roots to an active or partially initialized storage pool.
    pub(crate) fn first_storage_directory_parent(&self) -> Result<(&OpenAt2Root, (u32, u32))> {
        self.recheck()?;
        self.fresh()?;
        Ok((&self.disks, self.account.owner()))
    }

    /// Open only one explicitly selected immediate child through the retained
    /// discovery descriptor. Does not create, adopt, chmod or follow links.
    pub fn disk_root(&self, path: &str) -> Result<OpenAt2Root> {
        let names = all_names(&[path.to_owned()])?;
        let name = names.first().expect("invariant: one validated disk root");
        let file = self
            .disks
            .open_directory(&EntryName::new(name)?.as_relative())?;
        owner(&file, self.account.owner())?;
        self.recheck()?;
        Ok(OpenAt2Root::from_directory(file, MountPolicy::SameMount)?)
    }
    /// Require every actual direct child to be selected. Unknown, missing,
    /// foreign, extra or inaccessible material cannot become a hidden candidate.
    pub fn inventory(&self, paths: &[String]) -> Result<()> {
        names(paths)?;
        self.inventory_all(paths)
    }
    /// All current roots and the exact new batch, not just the new selections.
    pub fn inventory_all(&self, paths: &[String]) -> Result<()> {
        let mut missing = all_names(paths)?;
        for name in self.disks.entries()? {
            let name = name?;
            let text = name
                .as_os_str()
                .to_str()
                .ok_or("non-UTF-8 discovery material was preserved")?;
            if !missing.remove(text) {
                return Err("unselected or duplicate discovery material was preserved; the entire explicitly selected root set is required".into());
            }
            let file = self.disks.open_directory(&name.as_relative())?;
            owner(&file, self.account.owner())?;
        }
        if !missing.is_empty() {
            return Err(
                "all selected disk roots must already exist; no missing directory is created"
                    .into(),
            );
        }
        self.recheck()
    }

    /// Exact Add inventory after authenticated current registration has been
    /// read. Historical stopped names may be absent and are never opened.
    /// Every live/current or new root remains mandatory; unknown names reject.
    pub(crate) fn inventory_addition(&self, paths: &[String], stopped: &[PathBuf]) -> Result<()> {
        let mut missing = all_names(paths)?;
        let stopped_paths = stopped
            .iter()
            .map(|path| {
                path.to_str()
                    .map(str::to_owned)
                    .ok_or("stopped registration path is not UTF-8")
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let stopped_names = all_names(&stopped_paths)?;
        if !stopped_names.is_subset(&missing) {
            return Err(
                "stopped registration is outside the exact current addition root set".into(),
            );
        }
        missing.retain(|name| !stopped_names.contains(name));
        for name in self.disks.entries()? {
            let name = name?;
            let text = name
                .as_os_str()
                .to_str()
                .ok_or("non-UTF-8 discovery material was preserved")?;
            if stopped_names.contains(text) {
                continue;
            }
            if !missing.remove(text) {
                return Err("unselected discovery material was preserved; only authenticated historical roots and the exact new batch are allowed".into());
            }
            let path = format!("{DISKS}/{text}");
            let file = storage_server::storage_directories::open_unstopped_root_within(
                DISKS, &path, stopped,
            )?;
            owner(&file, self.account.owner())?;
        }
        if !missing.is_empty() {
            return Err(
                "all enabled and selected new roots must exist; missing roots were not recreated"
                    .into(),
            );
        }
        self.recheck()
    }
}
fn private(root: &OpenAt2Root, path: &str) -> Result<PrivateDirectory> {
    let directory = PrivateDirectory::from_directory_for_administration(
        root.directory().try_clone()?,
        path.into(),
    )?;
    directory.verify_identity()?;
    Ok(directory)
}
fn owner(file: &File, expected: (u32, u32)) -> Result<()> {
    let meta = file.metadata()?;
    if (meta.uid(), meta.gid()) != expected || meta.mode() & 0o7777 != 0o700 {
        return Err("managed storage requires exact service UID/GID and 0700 directories; existing permissions were not changed".into());
    }
    Ok(())
}
fn entries(root: &OpenAt2Root, allowed: &[&str]) -> Result<()> {
    for name in root.entries()? {
        let name = name?;
        if !allowed
            .iter()
            .any(|allowed| name.as_os_str() == std::ffi::OsStr::new(allowed))
        {
            return Err("unexpected installation material was preserved; all discovery children must be explicitly included and admissible".into());
        }
    }
    Ok(())
}
pub(crate) fn names(paths: &[String]) -> Result<BTreeSet<String>> {
    if paths.is_empty() || paths.len() > 32 {
        return Err(
            "select one to 32 existing empty roots for the first independent-primary batch".into(),
        );
    }
    all_names(paths)
}
fn all_names(paths: &[String]) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for path in paths {
        let path = Path::new(path);
        let leaf = path
            .strip_prefix(DISKS)
            .map_err(|_| "first-storage roots must be immediate children of /zxs/data/disks")?;
        let mut components = leaf.components();
        let name = match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) if path.parent() == Some(Path::new(DISKS)) => {
                name.to_str().ok_or("disk root must be UTF-8")?
            }
            _ => return Err("first-storage root is not one normalized direct child".into()),
        };
        if Path::new(DISKS).join(name).as_os_str() != path.as_os_str() || !names.insert(name.into())
        {
            return Err("first-storage roots must be normalized and distinct".into());
        }
    }
    Ok(names)
}
