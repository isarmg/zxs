use super::*;

impl Inventory {
    /// A killed atomic writer can leave a temporary inode inside a private,
    /// identity-bound replica. Preserve such extra files outside the proof tree;
    /// never skip or move a name that belongs to the reviewed source inventory.
    pub(super) fn quarantine_residue(
        &self,
        root: &DiskRoot,
        capsule: &DiskRoot,
        scope: &str,
        budget: &InspectionBudget,
    ) -> Result<()> {
        let actual = Self::capture(root, budget)?;
        if actual
            .directories
            .iter()
            .any(|path| !self.directories.contains(path))
        {
            return Err(changed("partial capsule contains unexpected directories"));
        }
        for path in actual
            .files
            .keys()
            .filter(|path| !self.files.contains_key(*path))
        {
            let name = EntryName::new(path.file_name().expect("invariant: inventory file leaf"))?;
            let payload_temp = name
                .as_os_str()
                .to_str()
                .and_then(|name| name.strip_prefix(".zxs-payload-"))
                .and_then(|name| name.strip_suffix(".tmp"))
                .is_some_and(|name| {
                    name.len() == 32
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                });
            if !payload_temp && !platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
                return Err(changed("partial capsule contains an unexpected file"));
            }
            let target = capsule.path().join("residue").join(scope).join(path);
            capsule.private_dir(target.parent().expect("invariant: residue has parent"))?;
            capsule.rename_noreplace_destination_first_synced(&root.path().join(path), &target)?;
        }
        Ok(())
    }

    pub(super) fn capture(root: &DiskRoot, budget: &InspectionBudget) -> Result<Self> {
        let mut inventory = Self {
            directories: BTreeSet::new(),
            files: BTreeMap::new(),
        };
        let mut remaining = budget.total_bytes();
        let mut pending = vec![PathBuf::new()];
        while let Some(relative) = pending.pop() {
            for entry in root.entries(&root.path().join(&relative))? {
                if inventory.files.len() + inventory.directories.len() >= MAX_ENTRIES {
                    return Err(Error::new(
                        ErrorCode::ResourceLimit,
                        "foreign replica entry budget exceeded",
                    ));
                }
                let path = relative.join(entry?.as_path());
                validate_path(&path)?;
                let absolute = root.path().join(&path);
                let metadata = root
                    .optional_metadata(&absolute)?
                    .ok_or_else(|| changed("source entry disappeared during inventory"))?;
                if metadata.is_dir() {
                    root.directory_identity(&absolute)?;
                    inventory.directories.insert(path.clone());
                    pending.push(path);
                } else if metadata.is_file() {
                    let mut file = root.open_read_regular(&absolute)?;
                    let bytes = file.metadata()?.len();
                    remaining = remaining.checked_sub(bytes).ok_or_else(|| {
                        Error::new(
                            ErrorCode::ResourceLimit,
                            "foreign replica byte budget exceeded",
                        )
                    })?;
                    let hash = hash_reader(&mut file, bytes)?;
                    inventory.files.insert(
                        path,
                        FileCommitment {
                            bytes,
                            digest: hash.to_hex().to_string(),
                        },
                    );
                } else {
                    return Err(Error::new(
                        ErrorCode::UnsafePath,
                        "foreign replica rejects links and special files",
                    ));
                }
            }
        }
        Ok(inventory)
    }

    pub(super) fn validate(&self, budget: &InspectionBudget) -> Result<()> {
        if self.files.len() + self.directories.len() > MAX_ENTRIES {
            return Err(invalid("capsule inventory exceeds entry limit"));
        }
        let mut remaining = budget.total_bytes();
        for path in self.directories.iter().chain(self.files.keys()) {
            validate_path(path)?;
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
                && !self.directories.contains(parent)
            {
                return Err(invalid("capsule inventory omits an ancestor directory"));
            }
        }
        for (path, file) in &self.files {
            if self.directories.contains(path) {
                return Err(invalid("capsule inventory has a file/directory collision"));
            }
            file.digest
                .parse::<ContentHash>()
                .map_err(|error| Error::from(std::io::Error::other(error)))?;
            remaining = remaining
                .checked_sub(file.bytes)
                .ok_or_else(|| invalid("capsule inventory exceeds byte limit"))?;
        }
        Ok(())
    }

    pub(super) fn copy(&self, source: &DiskRoot, target: &DiskRoot) -> Result<()> {
        for path in &self.directories {
            target.private_dir(&target.path().join(path))?;
        }
        for (path, file) in &self.files {
            let mut input = source.open_read_regular(&source.path().join(path))?;
            let hash: ContentHash = file
                .digest
                .parse()
                .expect("invariant: validated inventory digest");
            target.copy_verified_bytes_synced(
                &target.path().join(path),
                &mut input,
                platform_io::payload::Expectation {
                    bytes: file.bytes,
                    blake3: Some(hash.digest()),
                },
            )?;
        }
        target.sync_dir(target.path())
    }

    pub(super) fn verify(&self, root: &DiskRoot, budget: &InspectionBudget) -> Result<()> {
        if Self::capture(root, budget)? != *self {
            return Err(changed("capsule replica differs from the frozen inventory"));
        }
        Ok(())
    }

    pub(super) fn sync(&self, root: &DiskRoot) -> Result<()> {
        for path in self.files.keys() {
            root.open_regular(&root.path().join(path))?.sync_all()?;
        }
        // Ancestors sort before descendants; reverse traversal synchronizes
        // children before their names in each parent, including retry paths.
        for path in self.directories.iter().rev() {
            root.sync_dir(&root.path().join(path))?;
        }
        root.sync_dir(root.path())
    }
}

fn validate_path(path: &Path) -> Result<()> {
    RelativePath::new(path)?;
    let text = path
        .to_str()
        .ok_or_else(|| invalid("capsule paths must be UTF-8"))?;
    if text.len() > 4096 || path.components().count() > MAX_DEPTH {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "foreign replica path budget exceeded",
        ));
    }
    Ok(())
}

fn hash_reader(reader: &mut impl Read, expected: u64) -> Result<blake3::Hash> {
    let mut hash = blake3::Hasher::new();
    let mut buffer = [0; 64 * 1024];
    let mut remaining = expected;
    while remaining != 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..count])?;
        if read == 0 {
            return Err(changed("source file shortened during inventory"));
        }
        hash.update(&buffer[..read]);
        remaining -= read as u64;
    }
    if reader.read(&mut buffer[..1])? != 0 {
        return Err(changed("source file grew during inventory"));
    }
    Ok(hash.finalize())
}
