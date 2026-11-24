//! One explicit new workspace, one held native parent and one verified channel.
use super::{BasicLane, DB, Folder, Workspace};
use crate::{
    Result,
    r3::Catalog,
    session::{ExistingBasicLogin, ExistingBasicSession, Principal},
};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

/// The new child created under the expressly selected ordinary parent.
pub const NEW_WORKSPACE_DIRECTORY: &str = "zxs-workspace";

struct Destination {
    parent: Folder,
    path: PathBuf,
}

impl Destination {
    fn leaf() -> &'static OsStr {
        OsStr::new(NEW_WORKSPACE_DIRECTORY)
    }

    fn select(parent: &Path) -> Result<Self> {
        let held = Folder::open(parent, false)?;
        let choice = Self {
            path: held.path().join(Self::leaf()),
            parent: held,
        };
        crate::native::literal(&choice.path)?;
        choice.still_vacant()?;
        Ok(choice)
    }

    fn still_vacant(&self) -> Result<()> {
        self.parent.require_creation_parent()?;
        self.parent.absent(OsStr::new(DB))?; // Do not create a workspace inside a state directory.
        self.parent.absent(Self::leaf())
    }

    fn create(&self, permit: impl FnOnce() -> Result<()>) -> Result<Folder> {
        self.still_vacant()?;
        self.parent.new_private_child(Self::leaf(), permit)
    }
}

/// An exact new, absent private directory and one verified, unauthenticated
/// TLS/HTTP/2 channel. This choice neither creates files nor accepts a password.
pub struct NewWorkspace {
    destination: Destination,
    owner: Arc<()>,
    login: ExistingBasicLogin,
}

impl NewWorkspace {
    /// Select an absent child, then verify the native peer and H2 before asking for a password.
    ///
    /// # Errors
    /// Refuses an ineligible parent, an existing child, or a failed authenticated-transport check.
    pub async fn begin(parent: &Path, origin: &str, account: &str) -> Result<Self> {
        let destination = Destination::select(parent)?;
        let owner = Arc::new(());
        let login = ExistingBasicLogin::begin_new(Arc::clone(&owner), origin, account).await?;
        destination.still_vacant()?;
        Ok(Self {
            destination,
            owner,
            login,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.destination.path
    }

    #[must_use]
    pub fn principal(&self) -> &Principal {
        self.login.principal()
    }

    /// Verify just the first authenticated catalog on the original channel.
    /// A separate local cut (not another prompt) authorizes creation only after valid readback.
    ///
    /// # Errors
    /// Refuses a revoked choice, a changed parent/occupied name, an invalid catalog, or
    /// incomplete local setup. After the local cut a new directory or database may exist.
    pub async fn finish(
        self,
        password: Zeroizing<String>,
        admit_wire: impl Fn() -> Result<()>,
        admit_create: impl FnOnce() -> Result<()>,
    ) -> Result<(Workspace, ExistingBasicSession, Catalog)> {
        let Self {
            destination,
            owner,
            login,
        } = self;
        destination.still_vacant()?;
        let saved = login.principal().clone();
        let session = login.finish(&owner, &saved, password)?;
        let page = session
            .requests(|| {
                destination.still_vacant()?;
                admit_wire()
            })
            .catalog(None)
            .await?;
        let root = destination.create(admit_create)?;
        let workspace = Workspace::initialize_held(root, &saved, owner)?;
        workspace.validate_basic_lane(BasicLane::Existing(&session))?;
        Ok((workspace, session, page))
    }
}

#[cfg(test)]
#[path = "creation_tests.rs"]
mod tests;
