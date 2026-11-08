//! Collect only the already authenticated original publication prefix. The
//! caller must separately provide the actual post-prefix MembershipFence logs.
use super::*;

pub(in crate::engine) struct BackupNativeObjects {
    pub(in crate::engine) objects: BTreeMap<(String, String), Vec<u8>>,
    pub(in crate::engine) seals: BTreeMap<String, PublicationSeal>,
}
struct Collect<'a> {
    target: Target<'a>,
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

pub(in crate::engine::backup_preparation::native_control) fn collect(
    root: &DiskRoot,
    job: &Job,
    expected: &str,
    stop: &AtomicBool,
) -> Result<BackupNativeObjects> {
    let bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST)?;
    let manifest: Manifest = decode_proof(&bytes)?;
    if object_hash(FORMAT, &manifest)? != expected {
        return Err(invalid(
            "native generation objects belong to another original prefix",
        ));
    }
    let mut source = Collect {
        target: Target {
            root,
            stop,
            budget: Budget::default(),
        },
        objects: BTreeMap::new(),
    };
    verify(&manifest, &bytes, job, &mut source)?;
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
        objects: source.objects,
        seals,
    })
}
