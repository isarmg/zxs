use crate::{Result, private_io::BundleDestination};
use data_serialization::Serialize;
use platform_io::fs_safety::{
    AtomicCommitError, EntryName, Error,
    linux::{FileIdentity, MountPolicy, OpenAt2Root},
};
use rustix::{
    fs::{AtFlags, Mode, RenameFlags, fchown, mkdirat, renameat_with, statat},
    process::{Gid, Uid},
};
use std::{
    fs::File,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

struct Spec {
    name: &'static str,
    path: &'static str,
    kind: Kind,
}
enum Kind {
    Service,
    Export,
}
const SPECS: &[Spec] = &[
    Spec {
        name: "cache",
        path: "/zxs/cache",
        kind: Kind::Service,
    },
    Spec {
        name: "config",
        path: "/zxs/config",
        kind: Kind::Service,
    },
    Spec {
        name: "home",
        path: "/zxs/home",
        kind: Kind::Service,
    },
    Spec {
        name: "log",
        path: "/zxs/log",
        kind: Kind::Service,
    },
    Spec {
        name: "run",
        path: "/zxs/run",
        kind: Kind::Service,
    },
    Spec {
        name: "tmp",
        path: "/zxs/tmp",
        kind: Kind::Service,
    },
    Spec {
        name: "smb-export",
        path: "/zxs/smb-export",
        kind: Kind::Export,
    },
];

#[derive(Serialize)]
pub(super) struct Policy {
    destination: &'static str,
    uid: u32,
    gid: u32,
    mode: &'static str,
    empty_only: bool,
}

#[derive(Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
pub(super) enum Observation {
    Absent {
        destination: &'static str,
    },
    OriginalPending {
        destination: &'static str,
        device: String,
        inode: String,
    },
    Prepared {
        destination: &'static str,
        device: String,
        inode: String,
    },
}

enum Entry {
    Absent,
    Pending(OpenAt2Root),
    Prepared(OpenAt2Root),
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub(super) enum CompletionPhase {
    Absent,
    PendingBundle,
    FinalBundle,
}

pub(super) struct Directories {
    path: PathBuf,
    root: OpenAt2Root,
    service_owner: (u32, u32),
}
impl Directories {
    pub(super) fn open(
        path: &Path,
        host: &BundleDestination,
        service_owner: (u32, u32),
    ) -> Result<Self> {
        let root = OpenAt2Root::open(path, MountPolicy::SameMount)?;
        if root.identity() != host.identity() {
            return Err("runtime parent differs from the held host lease".into());
        }
        let result = Self {
            path: path.to_owned(),
            root,
            service_owner,
        };
        result.recheck()?;
        Ok(result)
    }

    pub(super) fn recheck(&self) -> Result<()> {
        let current = OpenAt2Root::open(&self.path, MountPolicy::SameMount)?;
        if current.identity() != self.root.identity() {
            return Err("runtime parent identity changed; no replacement path was accepted".into());
        }
        let metadata = current.directory().metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o755
        {
            return Err("runtime parent must be administrative-owned mode 0755 for separate SMB traversal; existing permissions were not changed".into());
        }
        Ok(())
    }

    fn attributes(&self, spec: &Spec) -> ((u32, u32), u32) {
        match spec.kind {
            Kind::Service => (self.service_owner, 0o700),
            Kind::Export => ((0, 0), 0o755),
        }
    }

    pub(super) fn policies(&self) -> Vec<Policy> {
        SPECS
            .iter()
            .map(|spec| {
                let (owner, _) = self.attributes(spec);
                Policy {
                    destination: spec.path,
                    uid: owner.0,
                    gid: owner.1,
                    mode: match spec.kind {
                        Kind::Service => "0700",
                        Kind::Export => "0755",
                    },
                    empty_only: true,
                }
            })
            .collect()
    }

    fn open_named(&self, name: &EntryName) -> Result<Option<OpenAt2Root>> {
        match self.root.open_directory(&name.as_relative()) {
            Ok(file) => Ok(Some(OpenAt2Root::from_directory(
                file,
                MountPolicy::SameMount,
            )?)),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn completion_phase(&self, name: &str) -> Result<CompletionPhase> {
        let final_directory = self.open_named(&EntryName::new(name)?)?;
        let pending_directory = self.open_named(&EntryName::new(format!("{name}.pending"))?)?;
        match (final_directory, pending_directory) {
            (None, None) => Ok(CompletionPhase::Absent),
            (None, Some(_)) => Ok(CompletionPhase::PendingBundle),
            (Some(_), None) => Ok(CompletionPhase::FinalBundle),
            (Some(_), Some(_)) => Err("both runtime completion names exist; preserve both".into()),
        }
    }

    fn check(&self, directory: &OpenAt2Root, spec: &Spec, pending: bool) -> Result<()> {
        let metadata = directory.directory().metadata()?;
        let (owner, mode) = self.attributes(spec);
        let final_attributes =
            (metadata.uid(), metadata.gid()) == owner && metadata.mode() & 0o7777 == mode;
        let initial_staging = pending && metadata.uid() == 0 && metadata.mode() & 0o7777 == 0o700;
        if !final_attributes && !initial_staging {
            return Err(format!(
                "{} has unexpected owner or permissions; it was not repaired",
                spec.path
            )
            .into());
        }
        if let Some(entry) = directory.entries()?.next() {
            let entry = entry?;
            return Err(format!(
                "{} is not empty ({:?}); used runtime/export data is never cleared",
                spec.path,
                entry.as_os_str()
            )
            .into());
        }
        Ok(())
    }

    fn entry(&self, spec: &Spec, digest: &str) -> Result<Entry> {
        let target = EntryName::new(spec.name)?;
        let pending = pending_name(spec, digest)?;
        let final_directory = self.open_named(&target)?;
        let pending_directory = self.open_named(&pending)?;
        match (final_directory, pending_directory) {
            (Some(_), Some(_)) => Err(format!(
                "{} and its original pending directory both exist; both were preserved",
                spec.path
            )
            .into()),
            (Some(directory), None) => {
                self.check(&directory, spec, false)?;
                check_named(&self.root, &target, directory.identity())?;
                Ok(Entry::Prepared(directory))
            }
            (None, Some(directory)) => {
                self.check(&directory, spec, true)?;
                check_named(&self.root, &pending, directory.identity())?;
                Ok(Entry::Pending(directory))
            }
            (None, None) => Ok(Entry::Absent),
        }
    }

    pub(super) fn inspect(&self, digest: &str) -> Result<Vec<Observation>> {
        self.recheck()?;
        SPECS
            .iter()
            .map(|spec| {
                Ok(match self.entry(spec, digest)? {
                    Entry::Absent => Observation::Absent {
                        destination: spec.path,
                    },
                    Entry::Pending(directory) => {
                        let identity = directory.identity();
                        Observation::OriginalPending {
                            destination: spec.path,
                            device: identity.device.to_string(),
                            inode: identity.inode.to_string(),
                        }
                    }
                    Entry::Prepared(directory) => {
                        let identity = directory.identity();
                        Observation::Prepared {
                            destination: spec.path,
                            device: identity.device.to_string(),
                            inode: identity.inode.to_string(),
                        }
                    }
                })
            })
            .collect()
    }

    pub(super) fn completed(&self, digest: &str) -> Result<Vec<Observation>> {
        let observations = self.inspect(digest)?;
        if observations
            .iter()
            .any(|entry| !matches!(entry, Observation::Prepared { .. }))
        {
            return Err("runtime preparation is not complete; at least one original directory is absent or pending".into());
        }
        Ok(observations)
    }

    pub(super) fn sync_completed(&self, digest: &str) -> Result<()> {
        self.completed(digest)?;
        for spec in SPECS {
            let Entry::Prepared(directory) = self.entry(spec, digest)? else {
                return Err("completed runtime directory disappeared; it was not recreated".into());
            };
            directory.directory().sync_all()?;
            check_named(
                &self.root,
                &EntryName::new(spec.name)?,
                directory.identity(),
            )?;
        }
        self.root.directory().sync_all()?;
        self.recheck()
    }

    pub(super) fn publish(
        &self,
        digest: &str,
        revalidate: impl Fn() -> Result<()>,
    ) -> Result<Vec<&'static str>> {
        self.inspect(digest)?;
        let mut created = Vec::new();
        for spec in SPECS {
            revalidate()?;
            if self.publish_one(spec, digest, &revalidate)? {
                created.push(spec.path);
            }
        }
        self.completed(digest)?;
        Ok(created)
    }

    fn publish_one(
        &self,
        spec: &Spec,
        digest: &str,
        revalidate: &impl Fn() -> Result<()>,
    ) -> Result<bool> {
        let pending = pending_name(spec, digest)?;
        let target = EntryName::new(spec.name)?;
        let directory = match self.entry(spec, digest)? {
            Entry::Prepared(directory) => {
                directory.directory().sync_all()?;
                self.root.directory().sync_all()?;
                self.check(&directory, spec, false)?;
                check_named(&self.root, &target, directory.identity())?;
                revalidate()?;
                return Ok(false);
            }
            Entry::Pending(directory) => directory,
            Entry::Absent => {
                mkdirat(
                    self.root.directory(),
                    pending.as_os_str(),
                    Mode::from_raw_mode(0o700),
                )?;
                let file = self.root.open_directory(&pending.as_relative())?;
                file.set_permissions(std::fs::Permissions::from_mode(0o700))?;
                OpenAt2Root::from_directory(file, MountPolicy::SameMount)?
            }
        };
        self.root.directory().sync_all()?;
        self.check(&directory, spec, true)?;
        check_named(&self.root, &pending, directory.identity())?;
        revalidate()?;
        let (owner, mode) = self.attributes(spec);
        set_attributes(directory.directory(), owner, mode)?;
        self.check(&directory, spec, false)?;
        directory.directory().sync_all()?;
        check_named(&self.root, &pending, directory.identity())?;
        revalidate()?;
        self.publish_name(spec, &pending, &target, &directory)?;
        self.check(&directory, spec, false)?;
        revalidate()?;
        Ok(true)
    }

    fn publish_name(
        &self,
        spec: &Spec,
        pending: &EntryName,
        target: &EntryName,
        directory: &OpenAt2Root,
    ) -> Result<()> {
        let failure = |source| PublicationFailure {
            destination: spec.path,
            parent: self.root.identity(),
            pending: pending.as_os_str().to_string_lossy().into_owned(),
            source,
        };
        renameat_with(
            self.root.directory(),
            pending.as_os_str(),
            self.root.directory(),
            target.as_os_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| failure(AtomicCommitError::Unknown(Error::Io(error.into()))))?;
        check_named(&self.root, target, directory.identity())
            .map_err(|error| failure(AtomicCommitError::Published(error)))?;
        self.root.directory().sync_all().map_err(|error| {
            failure(AtomicCommitError::Published(
                Error::PublishedDurabilityUnknown(error),
            ))
        })?;
        Ok(())
    }
}

fn pending_name(spec: &Spec, digest: &str) -> Result<EntryName> {
    Ok(EntryName::new(format!(
        ".zxs-helper-runtime-{digest}-{}.pending",
        spec.name
    ))?)
}

fn set_attributes(file: &File, owner: (u32, u32), mode: u32) -> Result<()> {
    let metadata = file.metadata()?;
    if (metadata.uid(), metadata.gid()) != owner {
        fchown(
            file,
            Some(Uid::from_raw(owner.0)),
            Some(Gid::from_raw(owner.1)),
        )?;
    }
    if metadata.mode() & 0o7777 != mode {
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn check_named(
    parent: &OpenAt2Root,
    name: &EntryName,
    identity: FileIdentity,
) -> std::result::Result<(), Error> {
    let stat = statat(
        parent.directory(),
        name.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(std::io::Error::from)?;
    if (stat.st_dev, stat.st_ino) != (identity.device, identity.inode) {
        return Err(Error::IdentityChanged(name.as_path().to_owned()));
    }
    Ok(())
}

#[derive(Debug)]
struct PublicationFailure {
    destination: &'static str,
    parent: FileIdentity,
    pending: String,
    source: AtomicCommitError,
}
impl std::fmt::Display for PublicationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "runtime directory publication to {} below {:?}, pending {}; preserve both names",
            self.destination, self.parent, self.pending
        )
    }
}
impl std::error::Error for PublicationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
#[path = "../../../../test/helper/unit/initialization/runtime_directory_tests.rs"]
mod tests;
