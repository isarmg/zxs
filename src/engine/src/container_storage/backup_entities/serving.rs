//! Complete entities at their published binding, separate from source-fence
//! inputs. This storage plan never admits membership or grants writer I/O.
use super::*;

pub(super) const PUBLISHED_ROOT: &str = "published-serving-image";

/// Values originate from the original authenticated native Publication and its
/// exact target registry. This descriptor alone does not establish authority.
#[derive(Serialize)]
pub(crate) struct PublishedImageBinding<'a> {
    pub(crate) source_fence_hash: &'a str,
    pub(crate) decision_hash: &'a str,
    pub(crate) publication_hash: &'a str,
    pub(crate) snapshot_hash: &'a str,
    pub(crate) registry: &'a crate::DiskRegistryCommitment,
    pub(crate) target: &'a crate::DiskRegistrationIdentity,
}

/// Before any new irreversible native choice, reserve a complete extra copy
/// plus the image planner's worst-case bounded metadata/directory allocation.
/// Existing choices resume with exact missing-file accounting instead; free
/// space is not a reservation against other processes or later I/O failures.
pub(crate) fn capacity_before_choice(body_bytes: u64) -> Result<u64> {
    body_bytes
        .checked_add(MAX_BYTES as u64)
        .and_then(|bytes| bytes.checked_add(MAX_MANIFEST as u64))
        .and_then(|bytes| bytes.checked_add(MAX_ENTRIES as u64 * 4096))
        .and_then(|bytes| bytes.checked_add(16 * 1024 * 1024))
        .ok_or_else(budget)
}

impl<'a> Image<'a> {
    /// Source payloads come only from this target's previously authenticated
    /// native packages. Databases come from this target's admitted PUBLISHED
    /// State, never the old source SQLite or an unchanged preparation database.
    pub(crate) fn capture_published(
        root: &DiskRoot,
        admitted: &'a AdmittedState,
        prefix: &[String],
        binding: PublishedImageBinding<'_>,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let mut image = Self::build(root, admitted, prefix, PUBLISHED_ROOT, stop)?;
        #[derive(Serialize)]
        struct Manifest<'a> {
            format: &'static str,
            binding: PublishedImageBinding<'a>,
            placement: &'a EntityPlacement,
            directories: &'a BTreeSet<PathBuf>,
            files: BTreeMap<&'a PathBuf, Descriptor>,
        }
        image.manifest = canonical_json(&Manifest {
            format: "zxs-backup-published-serving-image",
            binding,
            placement: &image.placement,
            directories: &image.directories,
            files: image
                .files
                .iter()
                .map(|(path, data)| (path, data.descriptor()))
                .collect(),
        })?;
        if image.manifest.len() > MAX_MANIFEST {
            return Err(budget());
        }
        Ok(image)
    }

    /// The actual owner address is determined by the complete, verified image.
    /// It cannot be supplied by a caller or borrowed from another target.
    pub(crate) fn file_home(&self, history: &str, file: &str, version: &str) -> Result<&Path> {
        self.placement
            .files
            .iter()
            .find(|home| {
                home.history_tree_id == history
                    && home.file_id == file
                    && home.version_ids.contains(version)
            })
            .map(|home| home.path.as_path())
            .ok_or_else(|| invalid("published file is outside its complete serving entity"))
    }
}
