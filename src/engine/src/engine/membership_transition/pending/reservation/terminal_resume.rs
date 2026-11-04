//! Terminal preparation uses the same directory-before-writer ownership and
//! immutable reservation namespace as Add, without opening any old member.
use super::*;

pub(in crate::engine::membership_transition) struct ResumeRoot {
    pub(in crate::engine::membership_transition) disk: DiskConfig,
    pub(in crate::engine::membership_transition) root: DiskRoot,
    pub(in crate::engine::membership_transition) identity: MemberIdentity,
    directory_lock: File,
}

impl ResumeRoot {
    pub(in crate::engine::membership_transition) fn hold(
        disk: DiskConfig,
        root: DiskRoot,
        target: &ValidatedConfig,
    ) -> Result<Self> {
        let directory_lock = root.directory(root.path())?;
        lock(&directory_lock)?;
        let physical = admission::verify_named_root(&root)?;
        Ok(Self {
            identity: admission::member_identity(target, &disk, physical),
            disk,
            root,
            directory_lock,
        })
    }

    pub(in crate::engine::membership_transition) fn inspect(
        &self,
        intent: &TransitionIntent,
        archive: &str,
    ) -> Result<()> {
        let name = name(
            &intent.origin,
            &intent.target,
            &intent.principal,
            intent.operation_id,
            &self.identity,
        )?;
        let seal = super::super::PendingDiskSeal {
            format: super::super::PendingFormat::MembershipPending,
            identity: self.identity.clone(),
            transition_hash: object_hash(INTENT_KIND, intent)?,
            target_config_hash: object_hash("membership-target", &intent.target)?,
            control_archive_root: archive.to_owned(),
        };
        if self
            .root
            .entries(self.root.path())?
            .next()
            .transpose()
            .map_err(fs_error)?
            .is_none()
        {
            return Ok(());
        }
        super::super::admission::inspect_new_root(&self.root, &seal, &name).map(|_| ())
    }

    /// Call only after the protected system resume binding has reached both
    /// durability barriers. Retrying repeats the barriers, including after an
    /// uncertain earlier mkdir, lock-file create or rename.
    pub(in crate::engine::membership_transition) fn reserve(
        self,
        intent: &TransitionIntent,
        archive: &str,
    ) -> Result<ReservedRoot> {
        self.inspect(intent, archive)?;
        let name = name(
            &intent.origin,
            &intent.target,
            &intent.principal,
            intent.operation_id,
            &self.identity,
        )?;
        match rustix::fs::mkdirat(
            &self.directory_lock,
            name.as_str(),
            rustix::fs::Mode::from_raw_mode(0o700),
        ) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => (),
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
        verify_marker(&self.root, &name)?;
        self.root.sync_dir(&self.root.path().join(&name))?;
        self.directory_lock.sync_all()?;
        let writer = self.root.lock_file()?;
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&writer).map_err(fs_error)?;
        lock(&writer)?;
        writer.sync_all()?;
        self.directory_lock.sync_all()?;
        let result = ReservedRoot {
            root: Arc::new(self.root),
            identity: self.identity,
            name,
            writer: crate::disk::FenceOwner::new(writer, Some(self.directory_lock)),
        };
        verify_writer(&result.root, &result.writer)?;
        Ok(result)
    }
}
