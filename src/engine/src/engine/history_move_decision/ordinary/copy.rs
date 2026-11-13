//! The durable origin binds the actual freshly copied inode before either
//! rename. Recovery never substitutes an equal-content or recreated object.
use super::*;
use crate::engine::history_move_target::OriginalMoveTarget;
use crate::history_package::preparation_path_exists as exists;

#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct Origin {
    format: String,
    candidate_hash: String,
    version_id: String,
    version_hash: String,
    original_copy: String,
    directory: Inode,
    body: Body,
}
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
struct Body {
    inode: Inode,
    modified_seconds: String,
    modified_nanoseconds: String,
}
impl Body {
    fn capture(file: &ReadFile, version: &FileVersion) -> Result<Self> {
        let inode = Inode::file(file, version)?;
        let metadata = file.metadata()?;
        Ok(Self {
            inode,
            modified_seconds: metadata.mtime().to_string(),
            modified_nanoseconds: metadata.mtime_nsec().to_string(),
        })
    }
}
struct Position {
    directory: PathBuf,
    body: PathBuf,
    installed_directory: bool,
    installed_body: bool,
}

impl store::Target {
    fn final_body(&self, version: &FileVersion) -> Result<PathBuf> {
        crate::container_storage::installed_version_path(&self.named, version)
    }
    fn origin(&self, version: &FileVersion) -> Result<Origin> {
        let stage = self.stage()?;
        let bytes = stage.read_bytes(&self.origin_path(version), MAX_RECORD)?;
        let origin: Origin = storage_contract::decode_strict(&bytes)?;
        if origin.format != "file-history-move-ordinary-origin"
            || origin.candidate_hash != self.candidate
            || origin.version_id != version.version_id
            || origin.version_hash != object_hash("version", version)?
            || origin.original_copy != self.copies[&version.version_id]
        {
            return Err(invalid(
                "ordinary copy origin differs from its original selected version",
            ));
        }
        Ok(origin)
    }
    fn position(&self, version: &FileVersion, origin: &Origin) -> Result<Position> {
        let root = &self.owner.root;
        let staged = self.staged_version(version);
        let installed = self.version_directory(version);
        let (directory, installed_directory) =
            match (exists(root, &staged)?, exists(root, &installed)?) {
                (true, false) => (staged, false),
                (false, true) => (installed, true),
                _ => {
                    return Err(invalid(
                        "ordinary version must retain exactly one original directory",
                    ));
                }
            };
        if Inode::directory(root, &directory)? != origin.directory {
            return Err(invalid(
                "ordinary version directory no longer has its original inode",
            ));
        }
        let source = directory.join("content").join(&version.creation_name);
        let target = self.final_body(version)?;
        let (body, installed_body) = match (exists(root, &source)?, exists(root, &target)?) {
            (true, false) => (source, false),
            (false, true) if installed_directory => (target, true),
            _ => {
                return Err(invalid(
                    "ordinary version must retain exactly one original independent body",
                ));
            }
        };
        if Body::capture(&root.open_read_regular(&body)?, version)? != origin.body {
            return Err(invalid(
                "ordinary move body was replaced; equal bytes cannot replace its origin",
            ));
        }
        Ok(Position {
            directory,
            body,
            installed_directory,
            installed_body,
        })
    }
    fn verify_position(
        &self,
        version: &FileVersion,
        origin: &Origin,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<Position> {
        self.verify_container()?;
        let position = self.position(version, origin)?;
        let root = &self.owner.root;
        let metadata = root.read_bytes(&position.directory.join("version.json"), MAX_RECORD)?;
        if metadata != canonical_json(version)?
            || root.read_bytes(&position.directory.join("payload.json"), MAX_RECORD)?
                != payload(version)?
        {
            return Err(invalid(
                "ordinary version metadata differs from the frozen candidate",
            ));
        }
        // Authenticate the held body itself, not a resolver that could select
        // another matching copy. Full chunk and whole-file proof stays bounded.
        let checks = read_authenticated_checksums_at(root, &position.directory, version)?;
        let manifest = decode_authenticated_checksums(&checks, version)?;
        let held = root.open_read_regular(&position.body)?;
        let before = crate::file_stamp::FileStamp::from_metadata(&held.metadata()?);
        if Body::capture(&held, version)? != origin.body {
            return Err(invalid(
                "ordinary body changed before complete byte verification",
            ));
        }
        let held = verify_open_payload_observed(held, manifest, version, &mut |_| {
            checkpoint(cancellation)
        })?;
        if crate::file_stamp::FileStamp::from_metadata(&held.metadata()?) != before
            || crate::file_stamp::FileStamp::from_metadata(
                &root.open_read_regular(&position.body)?.metadata()?,
            ) != before
        {
            return Err(invalid(
                "ordinary body changed during complete byte verification",
            ));
        }
        let after = self.position(version, origin)?;
        if after.directory != position.directory || after.body != position.body {
            return Err(invalid(
                "ordinary copy moved during complete byte verification",
            ));
        }
        self.check_owner()?;
        Ok(position)
    }
    pub(super) fn require_original_or_absent(
        &self,
        version: &FileVersion,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<bool> {
        let root = &self.owner.root;
        if exists(root, &self.origin_path(version))? {
            self.verify_position(version, &self.origin(version)?, cancellation)?;
            return Ok(false);
        }
        if exists(root, &self.staged_version(version))?
            || exists(root, &self.version_directory(version))?
            || exists(root, &self.final_body(version)?)?
        {
            return Err(invalid(
                "partial ordinary copy lacks its original inode record; all bytes retained",
            ));
        }
        Ok(true)
    }
    fn copy_new(
        &self,
        original: &OriginalMoveTarget,
        version: &FileVersion,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        let stage = self.stage()?;
        let directory = self.staged_version(version);
        stage.private_dir(&stage.path().join("versions"))?;
        stage.private_dir(&stage.path().join("origins"))?;
        let inode = stage.create_private_directory_synced(&directory)?;
        let held_directory = stage.private_child_matching(&directory, inode)?;
        held_directory.create_private_directory_synced(&directory.join("content"))?;
        let source = original.source(version, &self.copies[&version.version_id], cancellation)?;
        let body = directory.join("content").join(&version.creation_name);
        held_directory.write_atomic(
            &directory.join("version.json"),
            &canonical_json(version)?,
            false,
        )?;
        held_directory.write_atomic(&directory.join("payload.json"), &payload(version)?, false)?;
        let copied = verified_full_copy_at(
            &held_directory,
            Source {
                file: &source,
                cancellation,
            },
            &body,
            version,
            self.max_file_bytes(),
        )
        .map_err(CopyFailure::into_error)?;
        copied.check_at(&held_directory, &body)?;
        held_directory.write_atomic(
            &directory.join("checksums.bin"),
            &copied.checksums().encode()?,
            false,
        )?;
        let (_, held_body) = copied.into_parts();
        let origin = Origin {
            format: "file-history-move-ordinary-origin".into(),
            candidate_hash: self.candidate.clone(),
            version_id: version.version_id.clone(),
            version_hash: object_hash("version", version)?,
            original_copy: self.copies[&version.version_id].clone(),
            directory: Inode::directory(&held_directory, &directory)?,
            body: Body::capture(&held_body, version)?,
        };
        held_body.sync_all()?;
        held_directory.sync_dir(&directory.join("content"))?;
        held_directory.sync_dir(&directory)?;
        stage.sync_dir(&stage.path().join("versions"))?;
        // This marker precedes both renames. Never synthesize it on a retry
        // from an arbitrary preexisting body or a replacement directory.
        stage.write_atomic(
            &self.origin_path(version),
            &encode_metadata(&origin, MAX_RECORD)?,
            false,
        )?;
        stage.sync_dir(&stage.path().join("origins"))?;
        stage.sync_dir(stage.path())?;
        self.verify_position(version, &origin, cancellation)?;
        Ok(())
    }
    pub(super) fn prepare_version(
        &self,
        original: &OriginalMoveTarget,
        version: &FileVersion,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        checkpoint(cancellation)?;
        if self.require_original_or_absent(version, cancellation)? {
            self.copy_new(original, version, cancellation)?;
        }
        let origin = self.origin(version)?;
        let position = self.verify_position(version, &origin, cancellation)?;
        let root = &self.owner.root;
        if !position.installed_directory {
            let destination = self.version_directory(version);
            root.private_dir(
                destination
                    .parent()
                    .expect("invariant: version directory has a parent"),
            )?;
            root.rename_noreplace_destination_first_synced(&position.directory, &destination)?;
        }
        let position = self.verify_position(version, &origin, cancellation)?;
        if !position.installed_body {
            root.rename_noreplace_destination_first_synced(
                &position.body,
                &self.final_body(version)?,
            )?;
        }
        self.verify_version(version, cancellation)?;
        // Retry completes both barriers even after a prior rename returned an
        // uncertain error. Original source and original target stage never move.
        root.open_read_regular(&self.final_body(version)?)?
            .sync_all()?;
        root.sync_dir(&self.version_directory(version).join("content"))?;
        root.sync_dir(&self.version_directory(version))?;
        root.sync_dir(
            self.version_directory(version)
                .parent()
                .expect("invariant: version parent"),
        )?;
        root.sync_dir(
            self.staged_version(version)
                .parent()
                .expect("invariant: staging parent"),
        )?;
        root.sync_dir(&self.named.join("versions"))?;
        root.sync_dir(&self.named)?;
        let formal = self.version_directory(version);
        let history = formal
            .parent()
            .and_then(Path::parent)
            .expect("invariant: version has a history root");
        root.sync_dir(history)?;
        self.stage()?.sync_dir(self.stage()?.path())?;
        self.check_owner()
    }
    pub(super) fn verify_version(
        &self,
        version: &FileVersion,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        let position = self.verify_position(version, &self.origin(version)?, cancellation)?;
        if !position.installed_directory || !position.installed_body {
            return Err(invalid(
                "ordinary move copy has not reached both normal addresses",
            ));
        }
        Ok(())
    }
}

fn payload(version: &FileVersion) -> Result<Vec<u8>> {
    canonical_json(&json_codec::json!({
        "payload_id": format!("payload_{}", &version.version_id[4..]),
        "version_id": version.version_id, "kind": "FULL",
        "size": version.logical_size.to_string(), "content_hash": version.content_hash,
    }))
    .map_err(Into::into)
}
struct Source<'a> {
    file: &'a ReadFile,
    cancellation: &'a HistoryWindowCancellation,
}
impl FileExt for Source<'_> {
    fn read_at(&self, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
        checkpoint(self.cancellation).map_err(std::io::Error::other)?;
        let count = bytes.len().min(64 * 1024);
        self.file.read_at(&mut bytes[..count], offset)
    }
    fn write_at(&self, _: &[u8], _: u64) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ordinary move source is read-only",
        ))
    }
}
