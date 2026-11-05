//! New output trees are private, descriptor-bound and published without replace.
use super::*;
use platform_io::fs_safety::{
    EntryName, PrivateDirectory, RelativePath,
    linux::{FileIdentity, MountPolicy, OpenAt2Root},
};
use rustix::fs::{AtFlags, Mode, RenameFlags, mkdirat, renameat_with, statat};
use std::path::Component;

pub(super) struct Destination {
    path: PathBuf,
    parent: OpenAt2Root,
    name: EntryName,
}
impl Destination {
    pub fn admit(path: PathBuf) -> Result<Self> {
        let parent_path = path
            .parent()
            .ok_or_else(|| invalid("destination needs a parent"))?;
        let private = PrivateDirectory::open_existing(parent_path)?;
        let parent = OpenAt2Root::from_private_directory(private, MountPolicy::SameMount)?;
        let name = EntryName::new(
            path.file_name()
                .ok_or_else(|| invalid("destination needs a filename"))?,
        )?;
        absent(&parent, &name, &path)?;
        Ok(Self { path, parent, name })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn identity(&self) -> FileIdentity {
        self.parent.identity()
    }
    pub fn verify(&self) -> Result<()> {
        let private = PrivateDirectory::open_existing(
            self.path.parent().expect("invariant: destination parent"),
        )?;
        let parent = OpenAt2Root::from_private_directory(private, MountPolicy::SameMount)?;
        if parent.identity() != self.parent.identity() {
            return Err(invalid("destination parent identity changed"));
        }
        absent(&self.parent, &self.name, &self.path)
    }
    pub fn stage(&self) -> Result<StagedDirectory<'_>> {
        self.verify()?;
        let name = EntryName::new(format!(".zxs-convert-{}", crate::new_id("req")?))?;
        let path = self
            .path
            .parent()
            .expect("invariant: admitted parent")
            .join(name.as_path());
        mkdirat(
            self.parent.directory(),
            name.as_os_str(),
            Mode::from_raw_mode(0o700),
        )
        .map_err(|error| io("create conversion staging directory", &path, error.into()))?;
        let initialized: Result<_> = (|| {
            let file = self.parent.open_directory(&name.as_relative())?;
            file.sync_all()?;
            self.parent.directory().sync_all()?;
            let reader = OpenAt2Root::from_directory(file.try_clone()?, MountPolicy::SameMount)?;
            let directory = PrivateDirectory::from_directory(file, path.clone())?;
            Ok((directory, reader))
        })();
        let (directory, reader) = initialized.map_err(|source| {
            let code = source.code();
            source.context(code, format!("conversion staging initialization failed; preserve {path:?} and do not retry blindly"))
        })?;
        Ok(StagedDirectory {
            destination: self,
            directory,
            reader,
            name,
        })
    }
}
fn absent(parent: &OpenAt2Root, name: &EntryName, path: &Path) -> Result<()> {
    match statat(
        parent.directory(),
        name.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    ) {
        Err(rustix::io::Errno::NOENT) => Ok(()),
        Ok(_) => Err(Error::new(
            ErrorCode::DestinationExists,
            format!("conversion destination already exists: {path:?}"),
        )),
        Err(error) => Err(io("inspect destination", path, error.into())),
    }
}

pub(super) struct StagedDirectory<'a> {
    destination: &'a Destination,
    directory: PrivateDirectory,
    reader: OpenAt2Root,
    name: EntryName,
}
impl StagedDirectory<'_> {
    pub fn path(&self) -> &Path {
        self.directory.path()
    }
    pub fn create_directory(&self, path: &name_policy::PortablePath) -> Result<()> {
        let mut directory = self.root_directory()?;
        for part in Path::new(path.as_str()).components() {
            let Component::Normal(name) = part else {
                unreachable!("invariant: portable path components")
            };
            directory = directory.create_child(&EntryName::new(name)?)?;
        }
        Ok(())
    }
    fn root_directory(&self) -> Result<PrivateDirectory> {
        Ok(PrivateDirectory::from_directory(
            self.reader.directory().try_clone()?,
            self.path().to_owned(),
        )?)
    }
    pub fn file_parent(&self, path: &name_policy::PortablePath) -> Result<PrivateDirectory> {
        let Some(parent) = path.parent() else {
            return self.root_directory();
        };
        let directory = self
            .reader
            .open_directory(&RelativePath::new(parent.as_str())?)?;
        Ok(PrivateDirectory::from_directory(
            directory,
            self.path().join(parent.as_str()),
        )?)
    }
    pub fn publish(self) -> Result<()> {
        self.destination.verify()?;
        let named = self
            .destination
            .parent
            .open_directory(&self.name.as_relative())?;
        if FileIdentity::from_fd(&named)? != self.reader.identity() {
            return Err(invalid("conversion staging directory identity changed"));
        }
        self.directory.sync()?;
        renameat_with(
            self.destination.parent.directory(),
            self.name.as_os_str(),
            self.destination.parent.directory(),
            self.destination.name.as_os_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| {
            io(
                "publish converted directory without overwriting",
                &self.destination.path,
                error.into(),
            )
        })?;
        self.destination.parent.directory().sync_all().map_err(|source| {
            Error::from(source).context(ErrorCode::DurabilityUnknown,
                format!("converted directory was published at {:?}, but directory durability is uncertain; preserve it and do not retry", self.destination.path))
        })
    }
}
