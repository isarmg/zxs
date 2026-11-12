use super::*;
use crate::canonical_json;
use crate::journal::version_publication_fixture::Fixture;
use std::fs;

#[derive(Debug, PartialEq, Eq)]
enum Observation {
    Material(VersionProofMaterial),
    Membership,
}

/// A real-file test source with finite explicit locations. This deliberately
/// has no corresponding production constructor: relocation is not enabled.
struct SeparatedProofFiles {
    root: DiskRoot,
    publication: PathBuf,
    commit: PathBuf,
    observations: Vec<Observation>,
}

impl SeparatedProofFiles {
    fn from_fixture(fixture: &Fixture) -> Self {
        let root = DiskRoot::open(&fixture.engine.config().disks[0].root).unwrap();
        let directory = root.path().join("separated-proof-fixture");
        crate::disk::private_dir(&directory).unwrap();
        let publication = directory.join("original-publication");
        let commit = directory.join("original-commit");
        fs::rename(fixture.directory.join("publication-proof.json"), &publication).unwrap();
        fs::rename(fixture.directory.join("commit-proof.json"), &commit).unwrap();
        Self {
            root,
            publication,
            commit,
            observations: Vec::new(),
        }
    }
}

impl VersionPublicationRead for SeparatedProofFiles {
    fn read(&mut self, material: VersionProofMaterial) -> Result<Vec<u8>> {
        self.observations.push(Observation::Material(material));
        let path = match material {
            VersionProofMaterial::Publication => self.publication.clone(),
            VersionProofMaterial::Commit => self.commit.clone(),
            VersionProofMaterial::DiskIdentity => self.root.path().join("disk.json"),
        };
        self.root.read_bytes(&path, MAX_METADATA)
    }

    fn verify_membership(
        &mut self,
        candidate: &Candidate,
        seal: &DiskSeal,
        metadata: Option<&MetadataReadContext<'_>>,
    ) -> Result<()> {
        self.observations.push(Observation::Membership);
        verify_local_membership(
            self.root.path(),
            candidate,
            seal,
            &mut |path| self.root.read_bytes(path, MAX_METADATA),
            metadata,
        )
    }
}

fn retained_order() -> Vec<Observation> {
    vec![
        Observation::Material(VersionProofMaterial::Publication),
        Observation::Material(VersionProofMaterial::Commit),
        Observation::Material(VersionProofMaterial::DiskIdentity),
        Observation::Membership,
    ]
}

#[test]
fn original_proof_kernel_uses_explicit_files_without_version_ancestors() {
    let fixture = Fixture::new();
    let mut source = SeparatedProofFiles::from_fixture(&fixture);
    let publication = fs::read(&source.publication).unwrap();
    let commit = fs::read(&source.commit).unwrap();
    verify_local_version_publication(&fixture.version, &mut source, None).unwrap();
    assert_eq!(source.observations, retained_order());
    assert_eq!(fs::read(&source.publication).unwrap(), publication);
    assert_eq!(fs::read(&source.commit).unwrap(), commit);
    assert!(!fixture.directory.join("publication-proof.json").exists());
    assert!(!fixture.directory.join("commit-proof.json").exists());
    assert!(verify_local_publication(&fixture.directory, &fixture.version).is_err());
}

#[test]
fn separated_proof_keeps_exact_version_standalone_commit_and_full_route_receipts() {
    let fixture = Fixture::new();
    let mut source = SeparatedProofFiles::from_fixture(&fixture);
    let publication = fs::read(&source.publication).unwrap();
    let commit = fs::read(&source.commit).unwrap();
    let mut wrong_version = fixture.version.clone();
    wrong_version.content_hash = "0".repeat(64).parse().unwrap();
    assert_eq!(
        verify_local_version_publication(&wrong_version, &mut source, None)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );

    let mut changed: CommitProof = decode_proof(&commit).unwrap();
    changed.version_hash = "0".repeat(64);
    let damaged_commit = canonical_json(&changed).unwrap();
    fs::write(&source.commit, &damaged_commit).unwrap();
    assert_eq!(
        verify_local_version_publication(&fixture.version, &mut source, None)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );
    assert_eq!(fs::read(&source.commit).unwrap(), damaged_commit);
    fs::write(&source.commit, &commit).unwrap();

    let mut changed: LocalPublication = decode_proof(&publication).unwrap();
    assert_eq!(changed.commit_proof.route_slot_ids.len(), 3);
    assert_eq!(changed.publication.payload_receipts.len(), 3);
    changed.publication.payload_receipts.pop().unwrap();
    let damaged_publication = canonical_json(&changed).unwrap();
    fs::write(&source.publication, &damaged_publication).unwrap();
    assert_eq!(
        verify_local_version_publication(&fixture.version, &mut source, None)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );
    assert_eq!(fs::read(&source.publication).unwrap(), damaged_publication);
    fs::write(&source.publication, &publication).unwrap();
    verify_local_version_publication(&fixture.version, &mut source, None).unwrap();
}

#[test]
fn exterior_operation_result_is_not_stripped_or_accepted_with_old_receipts() {
    let fixture = Fixture::new();
    let mut source = SeparatedProofFiles::from_fixture(&fixture);
    let mut local: LocalPublication =
        decode_proof(&fs::read(&source.publication).unwrap()).unwrap();
    local.publication.result.result = json_codec::json!({
        "path": "outside/file.txt",
        "input": {"destination": "/external/directory", "extra": [null, 7]}
    });
    let bytes = canonical_json(&local).unwrap();
    let decoded: LocalPublication = decode_proof(&bytes).unwrap();
    assert_eq!(canonical_json(&decoded).unwrap(), bytes);
    assert_eq!(
        decoded.publication.result.result,
        local.publication.result.result
    );
    fs::write(&source.publication, &bytes).unwrap();
    assert_eq!(
        verify_local_version_publication(&fixture.version, &mut source, None)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );
    assert_eq!(fs::read(&source.publication).unwrap(), bytes);
}

#[test]
fn current_adapter_tombstone_presence_veto_is_not_tombstone_validation() {
    let fixture = Fixture::new();
    let root = DiskRoot::open(&fixture.engine.config().disks[0].root).unwrap();
    let tombstone = fixture.directory.join("purged.json");
    fs::write(&tombstone, b"not a valid tombstone").unwrap();
    verify_retained_version_publication_at(&root, &fixture.directory, &fixture.version, None)
        .unwrap();
    assert_eq!(
        verify_local_publication_at(
            &root,
            &fixture.directory,
            &fixture.version,
            &mut |_| Ok(()),
            None,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ContentPurged
    );
    fs::remove_file(&tombstone).unwrap();
    std::os::unix::fs::symlink("absent", &tombstone).unwrap();
    let error = verify_local_publication_at(
        &root,
        &fixture.directory,
        &fixture.version,
        &mut |_| Ok(()),
        None,
    )
    .unwrap_err();
    assert_ne!(error.code(), ErrorCode::ContentPurged);
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn retained_tombstone_rechecks_original_identity_then_its_own_membership() {
    let mut fixture = Fixture::new();
    let tombstone = fixture.purge();
    let mut source = SeparatedProofFiles::from_fixture(&fixture);
    verify_tombstone_content_with(&fixture.version, &tombstone, &mut source, None).unwrap();
    let mut expected = retained_order();
    expected.push(Observation::Material(VersionProofMaterial::DiskIdentity));
    expected.push(Observation::Membership);
    assert_eq!(source.observations, expected);

    let mut changed = fixture.version.clone();
    changed.content_hash = "0".repeat(64).parse().unwrap();
    source.observations.clear();
    assert_eq!(
        verify_tombstone_content_with(&changed, &tombstone, &mut source, None)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );
    assert_eq!(source.observations, retained_order());
}

#[test]
fn current_adapter_rejects_location_identity_before_observed_reads() {
    let fixture = Fixture::new();
    let root = DiskRoot::open(&fixture.engine.config().disks[0].root).unwrap();
    let wrong = fixture.directory.with_file_name("wrong-version");
    let mut observed = false;
    let error = verify_local_publication_at(
        &root,
        &wrong,
        &fixture.version,
        &mut |_| {
            observed = true;
            Ok(())
        },
        None,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PublicationInvalid);
    assert!(!observed);
}
