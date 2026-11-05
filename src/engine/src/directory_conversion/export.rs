use super::{output::Destination, *};
use crate::{Entry, NamespaceEntryKind};
use platform_io::fs_safety::EntryName;

/// Export is an explicitly selected namespace view, never a foreign-disk merge
/// or an assertion that ordinary directories can represent a complete history.
pub struct PreparedDirectoryExport {
    preview: DirectoryConversionPreview,
    destination: Destination,
    entries: Vec<Entry>,
    system: String,
    sequence: u64,
}
#[derive(Debug)]
pub struct DirectoryExportOutcome {
    pub destination: PathBuf,
    pub files: usize,
    pub directories: usize,
    pub content_bytes: u64,
}
impl Engine {
    pub fn prepare_directory_export(
        &self,
        branch: &str,
        commit: Option<&str>,
        destination: &Path,
    ) -> Result<PreparedDirectoryExport> {
        let destination = Destination::admit(external_root(self, destination)?)?;
        let entries = match commit {
            Some(commit) => self.list_tree_at_ref(branch, commit)?,
            None => self.list_tree_ref(branch)?,
        };
        let mut bytes = 0u64;
        for entry in entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            bytes = bytes
                .checked_add(entry.size)
                .ok_or_else(|| invalid("export byte count overflow"))?;
            self.open_content(&entry.version_id)?;
        }
        let mut digest = blake3::Hasher::new();
        digest.update(b"zxs:directory-export\0");
        for field in [
            self.config.pool_id.as_bytes(),
            branch.as_bytes(),
            commit.unwrap_or("").as_bytes(),
            destination.path().as_os_str().as_encoded_bytes(),
        ] {
            digest_field(&mut digest, field);
        }
        digest.update(&self.state.sequence.to_le_bytes());
        digest.update(&destination.identity().device.to_le_bytes());
        digest.update(&destination.identity().inode.to_le_bytes());
        for entry in entries {
            digest_field(&mut digest, entry.path.as_str().as_bytes());
            digest_field(&mut digest, entry.version_id.as_bytes());
            digest.update(&entry.size.to_le_bytes());
        }
        let preview = DirectoryConversionPreview {
            digest: DirectoryConversionDigest(*digest.finalize().as_bytes()),
            source: self.root.clone(),
            destination: destination
                .path()
                .to_str()
                .expect("invariant: admitted UTF-8 output")
                .into(),
            files: entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
                .count(),
            directories: entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                .count(),
            content_bytes: bytes,
            temporary_bytes: 0,
        };
        Ok(PreparedDirectoryExport {
            preview,
            destination,
            entries: entries.to_vec(),
            system: self.config.pool_id.clone(),
            sequence: self.state.sequence,
        })
    }
}
impl PreparedDirectoryExport {
    pub fn preview(&self) -> &DirectoryConversionPreview {
        &self.preview
    }
    pub fn execute(
        self,
        engine: &Engine,
        confirmation: DirectoryConversionConfirmation,
    ) -> Result<DirectoryExportOutcome> {
        confirmation.verify(&self.preview)?;
        if engine.config.pool_id != self.system || engine.state.sequence != self.sequence {
            return Err(Error::new(
                ErrorCode::PlanStale,
                "system changed; review a new export plan",
            ));
        }
        external_root(engine, self.destination.path())?;
        self.destination.verify()?;
        // Revalidate every source before publishing even a staging name. Each
        // actual copy below is independently length/hash checked while writing.
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            engine.open_content(&entry.version_id)?;
        }
        let staged = self.destination.stage()?;
        let staging_path = staged.path().to_owned();
        let copied: Result<()> = (|| {
            for entry in self
                .entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
            {
                staged.create_directory(&entry.path)?;
            }
            for entry in self
                .entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
            {
                let parent = staged.file_parent(&entry.path)?;
                let mut source = engine.open_content(&entry.version_id)?;
                platform_io::payload::write_full(
                    &parent,
                    &EntryName::new(entry.path.file_name())?,
                    &mut source.file,
                    platform_io::payload::Expectation {
                        bytes: entry.size,
                        blake3: Some(source.content_hash.digest()),
                    },
                )?;
            }
            staged.publish()
        })();
        copied.map_err(|source| {
            let code = source.code();
            source.context(code, format!("conversion output may be incomplete; preserve staging {:?} and destination {:?}; source data is unchanged", staging_path, self.destination.path()))
        })?;
        Ok(DirectoryExportOutcome {
            destination: self.destination.path().to_owned(),
            files: self.preview.files,
            directories: self.preview.directories,
            content_bytes: self.preview.content_bytes,
        })
    }
}
