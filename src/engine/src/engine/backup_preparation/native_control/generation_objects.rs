//! Collect only the already authenticated original publication prefix. The
//! caller must separately provide the actual post-prefix MembershipFence logs.
use super::*;
use crate::engine::backup_preparation::BackupPreparedTopologyNode;
use crate::engine::disk_catalog::FreshBackupBindings;

pub(in crate::engine) struct BackupNativeObjects {
    // In-memory join between the verified preparation manifest and every
    // original fenced member. No extra durable copy or new archive object.
    pub(in crate::engine) fixed_genesis: Vec<u8>,
    pub(in crate::engine) objects: BTreeMap<(String, String), Vec<u8>>,
    pub(in crate::engine) seals: BTreeMap<String, PublicationSeal>,
}
struct Collect<'a> {
    target: Target<'a, 'a>,
    objects: BTreeMap<(String, String), Vec<u8>>,
}
impl Source for Collect<'_> {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        let object = self.target.object(kind, hash)?;
        let key = (kind.to_owned(), hash.to_owned());
        match self.objects.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(object.bytes().to_vec());
            }
            std::collections::btree_map::Entry::Occupied(entry)
                if entry.get() == object.bytes() => {}
            _ => {
                return Err(invalid(
                    "original native object changed during generation capture",
                ));
            }
        }
        Ok(object)
    }
    fn journal(&mut self, index: usize, expected: &OriginalMember) -> Result<Vec<u8>> {
        self.target.journal(index, expected)
    }
    fn budget(&self) -> &Budget {
        self.target.budget()
    }
    fn account(&mut self, bytes: usize) -> Result<()> {
        self.target.account(bytes)
    }
}

pub(in crate::engine::backup_preparation::native_control) fn collect_selected(
    root: &DiskRoot,
    job: &Job,
    task: &NativeTask,
    selection: &[BackupPreparedTopologyNode],
    stop: &AtomicBool,
    encodings: Option<&ScopeEncodingMemo>,
) -> Result<BackupNativeObjects> {
    let bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST)?;
    let manifest: Manifest = decode_proof(&bytes)?;
    if object_hash(FORMAT, &manifest)? != task.archive_hash {
        return Err(invalid(
            "native generation objects belong to another original prefix",
        ));
    }
    let mut source = Collect {
        target: Target {
            root,
            stop,
            budget: Budget::default(),
            encodings: encodings.map(Encodings::Committed),
        },
        objects: BTreeMap::new(),
    };
    // This is the existing authenticated target-local pass BEFORE first
    // candidate preparation. Do not add a second archive traversal, or
    // infer a target's entire prehistory from the last empty projection.
    let mut fresh = None;
    verify_observed(&manifest, &bytes, job, &mut source, &mut |original, _, _, step| {
        if fresh.is_none() {
            fresh = Some(FreshBackupBindings::selected(original, selection)?);
        }
        fresh.as_ref()
            .expect("invariant: initialized original Backup bindings")
            .require_absent(step, stop)
    })?;
    let seals = manifest
        .publications
        .iter()
        .map(|published| {
            let bytes = source
                .objects
                .get(&("backup-publication-seal".into(), published.seal.clone()))
                .expect("invariant: verified prefix visited its publication seal");
            let seal: PublicationSeal = decode_proof(bytes)?;
            Ok((published.candidate.clone(), seal))
        })
        .collect::<Result<_>>()?;
    Ok(BackupNativeObjects {
        fixed_genesis: canonical_json(&manifest.config)?,
        objects: source.objects,
        seals,
    })
}
