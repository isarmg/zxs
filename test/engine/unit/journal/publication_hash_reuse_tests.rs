use super::*;

struct Fixture {
    _root: tempfile::TempDir,
    _engine: Engine,
    directory: PathBuf,
    version: FileVersion,
    local: LocalPublication,
    proof: CommitProof,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = Config::development([
            root.path().join("a"),
            root.path().join("b"),
            root.path().join("c"),
        ]);
        let disk = config.disks[0].clone();
        let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
        let repository = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("proof hashes").unwrap(),
                "P01",
                3,
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "owner")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", b"payload")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "owner".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            })
            .unwrap();
        let id = engine.list_tree(&repository.main_branch_id).unwrap()[0]
            .version_id
            .clone();
        let version = engine.state().versions[&id].clone();
        let directory = version_dir(&disk, &version.history_tree_id, &id);
        let local = read_json(&directory.join("publication-proof.json")).unwrap();
        let proof = read_json(&directory.join("commit-proof.json")).unwrap();
        Self {
            _root: root,
            _engine: engine,
            directory,
            version,
            local,
            proof,
        }
    }

    fn verify(&self, local: &LocalPublication, proof: &CommitProof) -> Result<()> {
        verify_local_version_publication(
            &self.directory,
            &self.version,
            &mut |path| {
                if path == self.directory.join("publication-proof.json") {
                    Ok(canonical_json(local)?)
                } else if path == self.directory.join("commit-proof.json") {
                    Ok(canonical_json(proof)?)
                } else {
                    read_bytes(path, MAX_METADATA)
                }
            },
            None,
        )
    }
}

fn assert_invalid(result: Result<()>, message: &str) {
    let error = result.unwrap_err();
    assert_eq!(error.code(), ErrorCode::PublicationInvalid);
    assert_eq!(error.message(), message);
}

// Rebind only the publication envelope after a test changes payload evidence.
fn reseal_publication(local: &mut LocalPublication) {
    let hash = object_hash("publication", &local.publication).unwrap();
    for receipt in &mut local.publication_receipts {
        receipt.record.object_hash = hash.clone();
        receipt.frame_hash = record_frame_hash(&receipt.record)
            .unwrap()
            .unwrap()
            .to_hex()
            .to_string();
    }
}

#[test]
fn borrowed_publication_receipts_match_seal_and_keep_input_unchanged() {
    let fixture = Fixture::new();
    let local = &fixture.local;
    let before = canonical_json(local).unwrap();
    let hashes = PublicationHashes::new(&local.candidate, &local.publication).unwrap();
    verify_publication_receipts(
        &local.candidate,
        &local.publication,
        &local.publication_receipts,
        &hashes,
    )
    .unwrap();
    let seal = PublicationSeal {
        candidate_hash: hashes.candidate,
        publication_hash: hashes.publication,
        receipts: local.publication_receipts.clone(),
    };
    verify_publication_seal(&local.candidate, &local.publication, &seal).unwrap();
    fixture.verify(local, &fixture.proof).unwrap();
    assert_eq!(canonical_json(local).unwrap(), before);
    for candidate_hash in [true, false] {
        let mut changed = seal.clone();
        if candidate_hash {
            changed.candidate_hash.push('0');
        } else {
            changed.publication_hash.push('0');
        }
        assert_invalid(
            verify_publication_seal(&local.candidate, &local.publication, &changed),
            "synchronized publication receipts from every configured member are required",
        );
    }
}

#[test]
fn borrowed_receipts_preserve_membership_chain_and_frame_rejections() {
    enum Tamper {
        MissingMember,
        ForeignMember,
        Accept,
        Finalize,
        Publication,
        Predecessor,
    }
    let fixture = Fixture::new();
    for case in [
        Tamper::MissingMember,
        Tamper::ForeignMember,
        Tamper::Accept,
        Tamper::Finalize,
        Tamper::Publication,
        Tamper::Predecessor,
    ] {
        let mut local = fixture.local.clone();
        let expected = match case {
            Tamper::MissingMember => {
                local.publication_receipts.pop().unwrap();
                "synchronized publication receipts from every configured member are required"
            }
            Tamper::ForeignMember => {
                local.publication_receipts[0].disk_id.push('x');
                "synchronized publication receipts from every configured member are required"
            }
            Tamper::Accept => {
                local
                    .publication
                    .finalize
                    .accepts
                    .iter_mut()
                    .find(|receipt| receipt.disk_id == local.publication_receipts[0].disk_id)
                    .unwrap()
                    .frame_hash
                    .push('x');
                "ACCEPT receipt differs"
            }
            Tamper::Finalize => {
                local
                    .publication
                    .finalize_receipts
                    .iter_mut()
                    .find(|receipt| receipt.disk_id == local.publication_receipts[0].disk_id)
                    .unwrap()
                    .record
                    .previous_frame_hash
                    .push('x');
                "member synchronized chain is invalid"
            }
            Tamper::Publication => {
                local.publication_receipts[0].frame_hash.push('x');
                "publication frame receipt is invalid"
            }
            Tamper::Predecessor => {
                local.publication_receipts[0]
                    .record
                    .previous_frame_hash
                    .push('x');
                "member synchronized chain is invalid"
            }
        };
        assert_invalid(fixture.verify(&local, &fixture.proof), expected);
    }
}

#[test]
fn local_proof_digests_preserve_each_binding_rejection() {
    enum Tamper {
        Candidate,
        Version,
        Disk,
        Slot,
        Finalize,
        Route,
        EmbeddedProof,
    }
    let fixture = Fixture::new();
    for case in [
        Tamper::Candidate,
        Tamper::Version,
        Tamper::Disk,
        Tamper::Slot,
        Tamper::Finalize,
        Tamper::Route,
        Tamper::EmbeddedProof,
    ] {
        let mut local = fixture.local.clone();
        let mut proof = fixture.proof.clone();
        match case {
            Tamper::Candidate => proof.candidate_hash.push('x'),
            Tamper::Version => proof.version_hash.push('x'),
            Tamper::Disk => proof.disk_id.push('x'),
            Tamper::Slot => proof.slot_id.push('x'),
            Tamper::Finalize => proof.finalize.candidate_hash.push('x'),
            Tamper::Route => proof.route_slot_ids.reverse(),
            Tamper::EmbeddedProof => local.commit_proof.version_hash.push('x'),
        }
        assert_invalid(
            fixture.verify(&local, &proof),
            "local proof does not bind the version",
        );
    }
    // Matching proof files still have to bind the admitted version and member.
    let mutations: [fn(&mut CommitProof); 4] = [
        |proof| proof.version_hash.push('x'),
        |proof| proof.disk_id.push('x'),
        |proof| proof.slot_id.push('x'),
        |proof| proof.finalize.candidate_hash.push('x'),
    ];
    for mutate in mutations {
        let mut local = fixture.local.clone();
        mutate(&mut local.commit_proof);
        assert_invalid(
            fixture.verify(&local, &local.commit_proof),
            "local proof does not bind the version",
        );
    }
}

#[test]
fn local_payload_and_route_checks_survive_digest_reuse() {
    enum Tamper {
        MissingLocal,
        Digest,
        MissingRoute,
        Placement,
    }
    let fixture = Fixture::new();
    for case in [
        Tamper::MissingLocal,
        Tamper::Digest,
        Tamper::MissingRoute,
        Tamper::Placement,
    ] {
        let mut local = fixture.local.clone();
        let selected = local
            .publication
            .payload_receipts
            .iter()
            .position(|receipt| {
                receipt.version_id == fixture.version.version_id
                    && receipt.disk_id == fixture.proof.disk_id
            })
            .unwrap();
        let expected = match case {
            Tamper::MissingLocal => {
                local.publication.payload_receipts.remove(selected);
                "local payload receipt absent"
            }
            Tamper::Digest => {
                local.publication.payload_receipts[selected]
                    .proof_hash
                    .push('x');
                "local receipt digest differs"
            }
            Tamper::MissingRoute => {
                let other = local
                    .publication
                    .payload_receipts
                    .iter()
                    .position(|receipt| receipt.disk_id != fixture.proof.disk_id)
                    .unwrap();
                local.publication.payload_receipts.remove(other);
                "a required route receipt is absent"
            }
            Tamper::Placement => {
                local.commit_proof.route_slot_ids.reverse();
                "candidate placement mismatch"
            }
        };
        reseal_publication(&mut local);
        assert_invalid(fixture.verify(&local, &local.commit_proof), expected);
    }
}

#[test]
fn local_verification_keeps_read_order_and_io_sources() {
    use std::error::Error as _;
    let fixture = Fixture::new();
    let root = fixture.directory.ancestors().nth(4).unwrap();
    let paths = [
        fixture.directory.join("publication-proof.json"),
        fixture.directory.join("commit-proof.json"),
        root.join("disk.json"),
        root.join("metadata/pool-membership/config.json"),
    ];
    let mut reads = Vec::new();
    verify_local_version_publication(
        &fixture.directory,
        &fixture.version,
        &mut |path| {
            reads.push(path.to_path_buf());
            read_bytes(path, MAX_METADATA)
        },
        None,
    )
    .unwrap();
    assert_eq!(reads, paths);
    for (index, failed) in paths.iter().enumerate() {
        let mut reads = Vec::new();
        let error = verify_local_version_publication(
            &fixture.directory,
            &fixture.version,
            &mut |path| {
                reads.push(path.to_path_buf());
                if path == failed {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "proof read denied",
                    )
                    .into())
                } else {
                    read_bytes(path, MAX_METADATA)
                }
            },
            None,
        )
        .unwrap_err();
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(source.to_string(), "proof read denied");
        assert_eq!(reads, paths[..=index]);
    }
}

#[test]
fn publication_hashing_keeps_encoding_failure_before_seal_mismatch() {
    let fixture = Fixture::new();
    let mut publication = fixture.local.publication.clone();
    publication.result.result = json_codec::json!(1.5);
    let expected = object_hash("publication", &publication).unwrap_err();
    let error = verify_publication_seal(
        &fixture.local.candidate,
        &publication,
        &PublicationSeal {
            candidate_hash: "incorrect".into(),
            publication_hash: "incorrect".into(),
            receipts: fixture.local.publication_receipts.clone(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), expected.code());
    assert_eq!(error.message(), expected.message());
}

#[test]
fn local_decode_path_and_tombstone_boundaries_are_not_cached() {
    let fixture = Fixture::new();
    let root = fixture.directory.ancestors().nth(4).unwrap();
    for failed in [
        fixture.directory.join("publication-proof.json"),
        fixture.directory.join("commit-proof.json"),
        root.join("disk.json"),
        root.join("metadata/pool-membership/config.json"),
    ] {
        let error = verify_local_version_publication(
            &fixture.directory,
            &fixture.version,
            &mut |path| {
                if path == failed {
                    Ok(b"{invalid".to_vec())
                } else {
                    read_bytes(path, MAX_METADATA)
                }
            },
            None,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidJson);
    }
    assert_invalid(
        verify_local_version_publication(
            &fixture.directory.with_file_name("different-version"),
            &fixture.version,
            &mut |_| panic!("invalid descriptor path must fail before reading"),
            None,
        ),
        "version directory identity differs from its descriptor",
    );
    fixture.verify(&fixture.local, &fixture.proof).unwrap();
    let error = verify_local_publication_with(
        &fixture.directory,
        &fixture.version,
        &mut |path| read_bytes(path, MAX_METADATA),
        &mut |path| {
            assert_eq!(path, fixture.directory.join("purged.json"));
            Ok(true)
        },
        None,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ContentPurged);
}
