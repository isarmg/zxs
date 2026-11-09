//! Native ordinary-address recovery recipe for already admitted data-only
//! Backup targets. No source marker, metadata journal or pool activation here.
use super::*;
use crate::engine::membership_transition::PreparedBackupControl;
use crate::history_package::preparation_checkpoint;

const MAX_FILES: usize = 65_536;
const MAX_BYTES: usize = 512 * 1024 * 1024;

pub(in crate::engine) struct PreparedBackupTargetLocator {
    config: ValidatedConfig,
    control: PreparedBackupControl,
    archive_path: PathBuf,
    files: BTreeMap<PathBuf, Vec<u8>>,
    bytes: usize,
    transition_hash: String,
}

fn limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "Backup target native locator exceeds 65536 files or 512 MiB",
    )
}

impl BackupWriterIoConfig {
    /// Only the sealed native writer capability can construct this recipe.
    /// The original voters stay in the archive, never become target journals.
    pub(in crate::engine) fn target_locator(
        &self,
        control: PreparedBackupControl,
        stop: &AtomicBool,
    ) -> Result<PreparedBackupTargetLocator> {
        let generation = self
            .config
            .generation()
            .expect("invariant: Backup writer capability has a native generation");
        let transition_hash = generation.portable.archive_transition()?;
        if control.root_hash() != generation.portable.archive_hash()?
            || object_hash(INTENT_KIND, &generation.intent)? != transition_hash
            || self.config.io_disks().any(|disk| disk.metadata_member)
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup locator changes the exact data-only target archive or original transition",
            ));
        }
        let archive_path = PathBuf::from("metadata/membership-pending")
            .join(&transition_hash)
            .join(CONTROL_ARCHIVE_DIRECTORY);
        let portable = canonical_json(&generation.portable)?;
        let mut files = BTreeMap::from([
            (
                PathBuf::from("metadata/pool-membership/config.json"),
                canonical_json(&generation.original)?,
            ),
            (PathBuf::from(GENERATION_FILE), portable.clone()),
            (
                PathBuf::from("metadata/membership-generations")
                    .join(&generation.hash)
                    .join("authority.json"),
                portable,
            ),
        ]);
        let mut count = files.len();
        let mut bytes = files.values().try_fold(0usize, |n, bytes| {
            n.checked_add(bytes.len()).ok_or_else(limit)
        })?;
        for (_, value) in control.files() {
            preparation_checkpoint(stop)?;
            count += 1;
            bytes = bytes.checked_add(value.len()).ok_or_else(limit)?;
            if count > MAX_FILES || bytes > MAX_BYTES {
                return Err(limit());
            }
        }
        control.visit_publication_proofs(stop, &mut |candidate, proof| {
            let encoded = canonical_json(proof)?;
            count += 1;
            bytes = bytes.checked_add(encoded.len()).ok_or_else(limit)?;
            if count > MAX_FILES || bytes > MAX_BYTES || encoded.len() > MAX_METADATA {
                return Err(limit());
            }
            files.insert(
                PathBuf::from("metadata/membership-history").join(format!("{candidate}.json")),
                encoded,
            );
            Ok(())
        })?;
        Ok(PreparedBackupTargetLocator {
            config: self.config.clone(),
            control,
            archive_path,
            files,
            bytes,
            transition_hash,
        })
    }
}

impl PreparedBackupTargetLocator {
    pub(in crate::engine) fn files(&self) -> impl Iterator<Item = (PathBuf, &[u8])> {
        self.files
            .iter()
            .map(|(path, bytes)| (path.clone(), bytes.as_slice()))
            .chain(
                self.control
                    .files()
                    .map(|(path, bytes)| (self.archive_path.join(path), bytes)),
            )
    }

    pub(in crate::engine) fn bytes(&self) -> usize {
        self.bytes
    }
    pub(in crate::engine) fn authority_hash(&self) -> &str {
        self.config
            .generation_hash()
            .expect("invariant: native target locator generation")
    }
    pub(in crate::engine) fn transition_hash(&self) -> &str {
        &self.transition_hash
    }
    pub(in crate::engine) fn archive_hash(&self) -> &str {
        self.control.root_hash()
    }

    /// The archive is replayed at the ordinary address on THIS target. Local
    /// configuration and first-candidate membership use ordinary cold readers,
    /// not isolated preparation paths or a caller-supplied success marker.
    pub(in crate::engine) fn verify_reader(
        &self,
        root: &DiskRoot,
        disk_id: &str,
        first: &Candidate,
        stop: &AtomicBool,
    ) -> Result<()> {
        preparation_checkpoint(stop)?;
        let disk = self
            .config
            .io_disks()
            .find(|disk| disk.disk_id == disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskNotFound,
                    "Backup locator reader is not an originally admitted target",
                )
            })?;
        if root.path() != disk.root {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Backup locator cannot relocate the original target address",
            ));
        }
        let seal = decode_seal(&root.read_bytes(&root.path().join("disk.json"), MAX_METADATA)?)?;
        validate_stored_seal(
            &seal,
            &identity_at(&self.config, disk, &root.directory(root.path())?)?,
        )?;
        self.control.verify(
            root.private_child(&root.path().join(&self.archive_path))?,
            stop,
        )?;
        let actual = super::super::verify_local_membership(
            root.path(),
            first,
            &seal,
            &mut |path| {
                preparation_checkpoint(stop)?;
                root.read_bytes(path, MAX_METADATA)
            },
            None,
        )?;
        if object_hash("backup-effective-config", &actual)?
            != object_hash("backup-effective-config", &self.config)?
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "ordinary Backup target locator changes the exact published Config",
            ));
        }
        preparation_checkpoint(stop)
    }
}
