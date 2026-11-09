//! Fixed installed-system identities for an offline administrator reset. No
//! discovery, implicit credential initialization, or Engine recovery is used.
use crate::offline::{
    Result,
    account::{LocalAccount, Name, Snapshot},
    private_io::BundleDestination,
};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    EntryName, PrivateDirectory,
    linux::{FileIdentity, MountPolicy, OpenAt2Root, SingleLinkRequirement},
};
use std::{fs::File, io::Read, os::unix::fs::MetadataExt, path::Path};
use storage_server::auth::AuthStore;

const DATA: &str = "/zxs/data";
const STATE: &str = "/zxs/data/state";
const MANAGEMENT: &str = "/zxs/data/management";
use storage_server::management::MANAGEMENT_AUTHORITY_BYTES as AUTHORITY;
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
    authority_marker: Identity,
    account: Snapshot,
}
pub(super) struct Target {
    pub auth: AuthStore,
    pub context: Context,
    account: LocalAccount,
    data: OpenAt2Root,
    state: OpenAt2Root,
    management: OpenAt2Root,
    // Release the global lease only after the credential writer and handles.
    pub host: BundleDestination,
}
impl Target {
    pub fn open(service_user: String) -> Result<Self> {
        if rustix::process::geteuid().as_raw() != 0 {
            return Err("administrator reset requires the manually run root helper".into());
        }
        let account = LocalAccount::resolve(Name::new(service_user)?)?;
        let host = BundleDestination::open(Path::new("/zxs"))?;
        let data = OpenAt2Root::open(Path::new(DATA), MountPolicy::AllowNestedMounts)?;
        check_owner(data.directory(), account.owner())?;
        let state = child(&data, "state", account.owner())?;
        let management = child(&data, "management", account.owner())?;
        let marker = authority_marker(&state, account.owner())?;
        let directory = PrivateDirectory::from_directory_for_administration(
            management.directory().try_clone()?,
            MANAGEMENT.into(),
        )?;
        let auth = AuthStore::open_system_management_directory(directory)?;
        let context = Context {
            host: host.identity().into(),
            data: data.identity().into(),
            state: state.identity().into(),
            management: management.identity().into(),
            authority_marker: marker.into(),
            account: account.snapshot(),
        };
        let target = Self {
            host,
            auth,
            context,
            account,
            data,
            state,
            management,
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
        ] {
            let current = OpenAt2Root::open(Path::new(path), MountPolicy::AllowNestedMounts)?;
            check_owner(current.directory(), self.account.owner())?;
            if current.identity() != held.identity() {
                return Err("installed credential directory changed identity; retain original operation evidence".into());
            }
        }
        if Identity::from(authority_marker(&self.state, self.account.owner())?)
            != self.context.authority_marker
        {
            return Err("management authority marker changed identity".into());
        }
        Ok(())
    }
}
fn child(parent: &OpenAt2Root, name: &str, owner: (u32, u32)) -> Result<OpenAt2Root> {
    let file = parent.open_directory(&EntryName::new(name)?.as_relative())?;
    check_owner(&file, owner)?;
    Ok(OpenAt2Root::from_directory(file, MountPolicy::SameMount)?)
}
fn check_owner(file: &File, owner: (u32, u32)) -> Result<()> {
    let metadata = file.metadata()?;
    if (metadata.uid(), metadata.gid()) != owner || metadata.mode() & 0o7777 != 0o700 {
        return Err("installed credential directories must retain the exact service UID/GID and 0700 mode; no ownership or permissions were repaired".into());
    }
    Ok(())
}
fn authority_marker(state: &OpenAt2Root, owner: (u32, u32)) -> Result<FileIdentity> {
    let name = EntryName::new("management-authority.json")?;
    let file = state.open_file(&name.as_relative(), false)?;
    let metadata = file.metadata()?;
    if (metadata.uid(), metadata.gid()) != owner
        || metadata.mode() & 0o7777 != 0o600
        || metadata.len() != AUTHORITY.len() as u64
    {
        return Err("existing system-management authority marker is missing, unsafe or unsupported; reset cannot initialize it".into());
    }
    let identity = SingleLinkRequirement::check(&file)?;
    let mut bytes = Vec::new();
    (&file)
        .take(AUTHORITY.len() as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes != AUTHORITY || state.identity_of(&name.as_relative())? != identity {
        return Err("existing system-management authority marker differs or changed; reset was not admitted".into());
    }
    Ok(identity)
}
