use super::{Binding, Identity, MAX_RECORD_BYTES, Target, bytes};
use crate::offline::Result;
use data_serialization::Serialize;
use platform_io::{
    fs_safety::{
        EntryName, Error,
        linux::{FileIdentity, MountPolicy, OpenAt2Root},
    },
    publication::{BundleDestination, BundleState},
};
use rustix::{
    fs::{AtFlags, Mode, RenameFlags, fchown, mkdirat, renameat_with, statat},
    process::{Gid, Uid},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

enum Location {
    Absent,
    Pending(OpenAt2Root),
    Complete(OpenAt2Root, Binding),
}
pub(super) struct Entry {
    path: String,
    location: Location,
}
#[derive(Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
pub(super) enum Observation {
    Absent {
        path: String,
    },
    OriginalPending {
        path: String,
        device: String,
        inode: String,
    },
    EmptyPrepared {
        path: String,
        device: String,
        inode: String,
    },
}
impl Entry {
    pub(super) fn is_absent(&self) -> bool {
        matches!(self.location, Location::Absent)
    }
    pub(super) fn completed(&self) -> Option<Binding> {
        match &self.location {
            Location::Complete(_, binding) => Some(binding.clone()),
            Location::Absent | Location::Pending(_) => None,
        }
    }
    pub(super) fn observation(&self) -> Observation {
        let path = self.path.clone();
        match &self.location {
            Location::Absent => Observation::Absent { path },
            Location::Pending(directory) => {
                let identity = directory.identity();
                Observation::OriginalPending {
                    path,
                    device: identity.device.to_string(),
                    inode: identity.inode.to_string(),
                }
            }
            Location::Complete(directory, _) => {
                let identity = directory.identity();
                Observation::EmptyPrepared {
                    path,
                    device: identity.device.to_string(),
                    inode: identity.inode.to_string(),
                }
            }
        }
    }
}
fn name(path: &str) -> Result<EntryName> {
    Ok(EntryName::new(
        Path::new(path)
            .file_name()
            .expect("invariant: first-storage roots were normalized"),
    )?)
}
fn names(target: &Target, digest: &str) -> Result<(EntryName, String)> {
    let leaf = platform_io::lower_hex(&Sha256::digest(target.path.as_bytes()));
    let record = format!(".zxs-helper-first-root-{digest}-{leaf}");
    Ok((EntryName::new(format!("{record}.pending"))?, record))
}
fn named(parent: &OpenAt2Root, entry: &EntryName, identity: FileIdentity) -> Result<()> {
    let stat = statat(
        parent.directory(),
        entry.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )?;
    if (stat.st_dev, stat.st_ino) != (identity.device, identity.inode) {
        return Err("first-storage directory name changed its original inode".into());
    }
    Ok(())
}
fn open(parent: &OpenAt2Root, entry: &EntryName) -> Result<Option<OpenAt2Root>> {
    match parent.open_directory(&entry.as_relative()) {
        Ok(file) => Ok(Some(OpenAt2Root::from_directory(
            file,
            MountPolicy::SameMount,
        )?)),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn empty(directory: &OpenAt2Root) -> Result<()> {
    if let Some(entry) = directory.entries()?.next() {
        let _ = entry?;
        return Err("first-storage directory is not empty; no content was removed".into());
    }
    Ok(())
}
fn attributes(directory: &OpenAt2Root, owner: (u32, u32), pending: bool) -> Result<bool> {
    empty(directory)?;
    let metadata = directory.directory().metadata()?;
    if (metadata.uid(), metadata.gid()) == owner && metadata.mode() & 0o7777 == 0o700 {
        return Ok(false);
    }
    // Only the original administrative staging inode may finish mkdir/umask
    // and ownership handoff. A published or service-owned path is never repaired.
    if pending && metadata.uid() == 0 && metadata.mode() & 0o7077 == 0 {
        return Ok(true);
    }
    Err("first-storage directory has different ownership or permissions; existing paths were preserved".into())
}
fn inventory(parent: &OpenAt2Root, allowed: &BTreeSet<std::ffi::OsString>) -> Result<()> {
    for entry in parent.entries()? {
        if !allowed.contains(entry?.as_os_str()) {
            return Err("unselected first-storage discovery material was preserved".into());
        }
    }
    Ok(())
}
pub(super) fn capture(
    parent: &OpenAt2Root,
    owner: (u32, u32),
    paths: &[String],
) -> Result<Vec<Target>> {
    let mut allowed = BTreeSet::new();
    let mut targets = Vec::with_capacity(paths.len());
    for path in paths {
        let entry = name(path)?;
        allowed.insert(entry.as_os_str().to_owned());
        let original = match open(parent, &entry)? {
            None => None,
            Some(directory) => {
                attributes(&directory, owner, false)?;
                named(parent, &entry, directory.identity())?;
                Some(directory.identity().into())
            }
        };
        targets.push(Target {
            path: path.clone(),
            original,
        });
    }
    inventory(parent, &allowed)?;
    Ok(targets)
}
fn binding(
    host: &BundleDestination,
    target: &Target,
    digest: &str,
    record: &str,
) -> Result<(Option<Binding>, BundleState)> {
    let retained = host.read_operation(record, "inode.json", MAX_RECORD_BYTES)?;
    let value = retained
        .map(|bytes| json_codec::from_slice::<Binding>(&bytes))
        .transpose()?;
    if let Some(value) = &value
        && (value.request_digest != digest || value.path != target.path)
    {
        return Err("first-storage inode record belongs to a different original request".into());
    }
    let encoded = value.as_ref().map(bytes).transpose()?.unwrap_or_default();
    let phase = host.inspect(record, &[("inode.json", &encoded)])?;
    Ok((value, phase))
}
fn observe_one(
    host: &BundleDestination,
    parent: &OpenAt2Root,
    owner: (u32, u32),
    target: &Target,
    digest: &str,
) -> Result<Entry> {
    let final_name = name(&target.path)?;
    let (pending_name, record) = names(target, digest)?;
    let final_directory = open(parent, &final_name)?;
    let pending_directory = open(parent, &pending_name)?;
    let location = if let Some(original) = &target.original {
        if pending_directory.is_some() {
            return Err("an existing selected root has unexpected staging material".into());
        }
        let directory = final_directory.ok_or("original existing first-storage root disappeared")?;
        attributes(&directory, owner, false)?;
        if Identity::from(directory.identity()) != *original {
            return Err("original existing first-storage root was replaced".into());
        }
        named(parent, &final_name, directory.identity())?;
        Location::Complete(
            directory,
            Binding {
                request_digest: digest.into(),
                path: target.path.clone(),
                identity: original.clone(),
            },
        )
    } else {
        let (binding, phase) = binding(host, target, digest, &record)?;
        match (final_directory, pending_directory) {
            (Some(_), Some(_)) => {
                return Err("both final and pending first-storage roots exist; neither was changed".into());
            }
            (Some(directory), None) => {
                let binding = binding.ok_or("published new root has no original inode binding")?;
                if phase != BundleState::Published
                    || Identity::from(directory.identity()) != binding.identity
                {
                    return Err("published new root does not match its durable original inode binding".into());
                }
                attributes(&directory, owner, false)?;
                named(parent, &final_name, directory.identity())?;
                Location::Complete(directory, binding)
            }
            (None, Some(directory)) => {
                let administrative = attributes(&directory, owner, true)?;
                if let Some(binding) = &binding
                    && Identity::from(directory.identity()) != binding.identity
                {
                    return Err("pending first-storage root no longer has its original inode".into());
                }
                if binding.is_none() && !administrative {
                    return Err("handed-off pending root has no original inode record".into());
                }
                if !administrative && phase != BundleState::Published {
                    return Err("pending root ownership changed before its original inode was durably recorded".into());
                }
                named(parent, &pending_name, directory.identity())?;
                Location::Pending(directory)
            }
            (None, None) => {
                if binding.is_some() || phase != BundleState::Absent {
                    return Err("an original prepared root disappeared; it was not recreated".into());
                }
                Location::Absent
            }
        }
    };
    Ok(Entry {
        path: target.path.clone(),
        location,
    })
}
pub(super) fn observe(
    host: &BundleDestination,
    parent: &OpenAt2Root,
    owner: (u32, u32),
    targets: &[Target],
    digest: &str,
) -> Result<Vec<Entry>> {
    let mut allowed = BTreeSet::new();
    for target in targets {
        let (pending, _) = names(target, digest)?;
        if !allowed.insert(name(&target.path)?.as_os_str().to_owned())
            || !allowed.insert(pending.as_os_str().to_owned())
        {
            return Err("selected and pending first-storage root names overlap".into());
        }
    }
    inventory(parent, &allowed)?;
    targets
        .iter()
        .map(|target| observe_one(host, parent, owner, target, digest))
        .collect()
}
pub(super) fn publish(
    host: &BundleDestination,
    parent: &OpenAt2Root,
    owner: (u32, u32),
    target: &Target,
    digest: &str,
    revalidate: impl Fn() -> Result<()>,
) -> Result<()> {
    revalidate()?;
    let observed = observe_one(host, parent, owner, target, digest)?;
    let (pending, record) = names(target, digest)?;
    let directory = match observed.location {
        Location::Complete(directory, _) => {
            directory.directory().sync_all()?;
            parent.directory().sync_all()?;
            named(parent, &name(&target.path)?, directory.identity())?;
            return revalidate();
        }
        Location::Pending(directory) => directory,
        Location::Absent => {
            mkdirat(
                parent.directory(),
                pending.as_os_str(),
                Mode::from_raw_mode(0o700),
            )?;
            let directory = open(parent, &pending)?
                .ok_or("new first-storage pending directory disappeared")?;
            parent.directory().sync_all()?;
            directory
        }
    };
    attributes(&directory, owner, true)?;
    let binding = Binding {
        request_digest: digest.into(),
        path: target.path.clone(),
        identity: directory.identity().into(),
    };
    named(parent, &pending, directory.identity())?;
    directory.directory().sync_all()?;
    parent.directory().sync_all()?;
    revalidate()?;
    let encoded = bytes(&binding)?;
    host.publish(&record, &[("inode.json", &encoded)])?;
    named(parent, &pending, directory.identity())?;
    revalidate()?;
    if attributes(&directory, owner, true)? {
        directory
            .directory()
            .set_permissions(std::fs::Permissions::from_mode(0o700))?;
        fchown(
            directory.directory(),
            Some(Uid::from_raw(owner.0)),
            Some(Gid::from_raw(owner.1)),
        )?;
    }
    attributes(&directory, owner, false)?;
    directory.directory().sync_all()?;
    named(parent, &pending, directory.identity())?;
    revalidate()?;
    let final_name = name(&target.path)?;
    renameat_with(
        parent.directory(),
        pending.as_os_str(),
        parent.directory(),
        final_name.as_os_str(),
        RenameFlags::NOREPLACE,
    )?;
    named(parent, &final_name, directory.identity())?;
    parent.directory().sync_all()?;
    attributes(&directory, owner, false)?;
    revalidate()
}
