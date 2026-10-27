use super::*;

// Frozen R29 (6da614ccb133b3f036f0be52d815d11519d06f76) owned schema.
// Keep this independent of LocalPublication's new Serialize delegation.
mod r29_local_publication {
    use super::*;

    #[derive(data_serialization::Serialize)]
    struct LocalPublication {
        candidate: Candidate,
        publication: Publication,
        commit_proof: CommitProof,
        publication_receipts: Vec<Receipt>,
    }

    pub(super) fn encode(local: &super::LocalPublication, maximum: usize) -> Result<Vec<u8>> {
        encode_metadata(
            &LocalPublication {
                candidate: local.candidate.clone(),
                publication: local.publication.clone(),
                commit_proof: local.commit_proof.clone(),
                publication_receipts: local.publication_receipts.clone(),
            },
            maximum,
        )
    }
}

fn assert_local_publication_encoding(local: &LocalPublication) -> Vec<u8> {
    let expected = r29_local_publication::encode(local, MAX_METADATA).unwrap();
    let view = LocalPublicationRef {
        candidate: &local.candidate,
        publication: &local.publication,
        commit_proof: &local.commit_proof,
        publication_receipts: &local.publication_receipts,
    };
    assert_eq!(canonical_json(&view).unwrap(), expected);
    assert_eq!(canonical_json(local).unwrap(), expected);
    for maximum in [expected.len(), expected.len() + 1] {
        assert_eq!(encode_metadata(&view, maximum).unwrap(), expected);
        assert_eq!(encode_metadata(local, maximum).unwrap(), expected);
    }
    let maximum = expected.len() - 1;
    let old_error = r29_local_publication::encode(local, maximum).unwrap_err();
    for new_error in [
        encode_metadata(&view, maximum).unwrap_err(),
        encode_metadata(local, maximum).unwrap_err(),
    ] {
        assert_eq!(new_error.code(), old_error.code());
        assert_eq!(new_error.to_string(), old_error.to_string());
    }
    let decoded: LocalPublication = decode_strict(&expected).unwrap();
    assert_eq!(
        r29_local_publication::encode(&decoded, MAX_METADATA).unwrap(),
        expected
    );
    expected
}

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("容量边界").unwrap(),
            "P01",
            3,
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    (temp, engine, repo)
}
#[test]
fn metadata_graph_limit_rejects_before_any_new_payload_or_decision_and_keeps_published_state() {
    let (_temp, mut engine, repo) = fixture();
    let before = canonical_json(engine.state()).unwrap();
    let snapshot = engine.snapshot.clone();
    let next_slot = engine.next_slot;
    let mut next = engine.state().clone();
    next.sequence = next_slot;
    // A realistic next tree grows the history while preserving the original snapshot.
    let original = next.commits[&next.branches[&repo.main_branch_id].head_commit_id].clone();
    let source = engine.root.join("unpublished-full-source");
    std::fs::write(&source, b"prepared complete bytes").unwrap();
    let mut pending = Vec::new();
    let version = engine
        .build_version(
            &mut next,
            &mut pending,
            crate::business::VersionBuild {
                history: &repo.history_tree_id,
                file_id: &new_id("file"),
                path: &name_policy::PortablePath::parse("pending.txt").unwrap(),
                source: &source,
                primary: "P01",
                copies: 3,
                source_ids: Vec::new(),
                checkpoint: false,
            },
        )
        .unwrap();
    let mut commit = original.clone();
    commit.commit_id = new_id("commit");
    commit.parent_commit_ids = vec![original.commit_id];
    commit.entries.push(Entry {
        entry_id: new_id("entry"),
        path: name_policy::PortablePath::parse("pending.txt").unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: version.file_id.clone(),
        version_id: version.version_id.clone(),
        size: version.logical_size,
    });
    commit.kind = "content".into();
    commit.sequence = next_slot;
    next.branches
        .get_mut(&repo.main_branch_id)
        .unwrap()
        .head_commit_id = commit.commit_id.clone();
    next.commits.insert(commit.commit_id.clone(), commit);
    let mut result = next.operations.values().next().unwrap().clone();
    result.operation_id = storage_contract::OperationId::generate();
    result.sequence = next_slot;
    next.operations.insert(result.operation_id, result.clone());
    // Derive the exact successful boundary using the same serializer.
    let mut high = 1usize;
    while crate::history_package::store_history_manifests_bounded(&[], &next, high).is_err() {
        high *= 2;
    }
    let mut low = 0usize;
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        if crate::history_package::store_history_manifests_bounded(&[], &next, mid).is_ok() {
            high = mid;
        } else {
            low = mid;
        }
    }
    assert!(crate::history_package::store_history_manifests_bounded(&[], &next, high).is_ok());
    // The fixed Snapshot root can now exceed the smaller history root. The
    // rejection contract is independent of which metadata object is checked
    // first: this limit is already proven insufficient for a real history object.
    assert!(crate::history_package::store_history_manifests_bounded(&[], &next, high - 1).is_err());
    let logs = engine
        .members()
        .iter()
        .map(|d| fs_bytes(&log_path(d)))
        .collect::<Vec<_>>();
    let error = engine
        .persist_with_metadata_limit(next, pending, result.clone(), high - 1)
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(engine.next_slot, next_slot);
    assert!(!engine.blocked);
    for disk in &engine.config.disks {
        assert!(!version_dir(disk, &repo.history_tree_id, &version.version_id).exists());
    }
    assert_eq!(
        engine.operation(result.operation_id).unwrap_err().code(),
        storage_contract::ErrorCode::OperationNotFound
    );
    assert_eq!(
        engine
            .members()
            .iter()
            .map(|d| fs_bytes(&log_path(d)))
            .collect::<Vec<_>>(),
        logs
    );
}
fn fs_bytes(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}
#[test]
fn actual_publication_and_local_proofs_fit_the_exact_preflight_boundary() {
    let (_temp, mut engine, repo) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"whole bytes")
        .unwrap();
    let result = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    let disk = engine.members()[0].clone();
    let scanned = scan(&disk).unwrap();
    let (record, _) = scanned.records.last().unwrap();
    let candidate: Candidate = load_object(&disk, "candidate", &record.candidate_hash).unwrap();
    let publication: Publication = load_object(&disk, "publication", &record.object_hash).unwrap();
    let largest = preflight_publication(
        &engine,
        &candidate,
        &result,
        &engine.state,
        None,
        MAX_METADATA,
    )
    .unwrap();
    let mut actual = vec![
        canonical_json(&candidate).unwrap().len(),
        canonical_json(&publication).unwrap().len(),
        canonical_json(&publication.finalize).unwrap().len(),
        fs_bytes(&publication_seal_path(&disk, &record.candidate_hash)).len(),
    ];
    for version in &candidate.versions {
        for slot in &version.placement.route_slot_ids {
            let dir = version_dir(
                engine.disk(slot).unwrap(),
                &version.version.history_tree_id,
                &version.version.version_id,
            );
            actual.push(fs_bytes(&dir.join("commit-proof.json")).len());
            let bytes = fs_bytes(&dir.join("publication-proof.json"));
            let local: LocalPublication = decode_strict(&bytes).unwrap();
            assert_eq!(assert_local_publication_encoding(&local), bytes);
            verify_local_publication(&dir, &version.version).unwrap();
            actual.push(bytes.len());
        }
    }
    assert_eq!(largest, *actual.iter().max().unwrap());
    assert_eq!(
        preflight_publication(&engine, &candidate, &result, &engine.state, None, largest).unwrap(),
        largest
    );
    assert_eq!(
        preflight_publication(
            &engine,
            &candidate,
            &result,
            &engine.state,
            None,
            largest - 1
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    // The actual final sequence occupies all twenty decimal digits.
    let mut wide = candidate;
    wide.sequence = u64::MAX;
    let mut result = result;
    result.sequence = u64::MAX;
    let largest =
        preflight_publication(&engine, &wide, &result, &engine.state, None, MAX_METADATA).unwrap();
    assert!(preflight_publication(&engine, &wide, &result, &engine.state, None, largest).is_ok());
    assert!(
        preflight_publication(&engine, &wide, &result, &engine.state, None, largest - 1).is_err()
    );
}
fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> String {
    let workspace = engine.create_workspace(branch, "alice").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    engine.list_tree(branch).unwrap()[0].version_id.clone()
}

#[test]
fn local_publication_borrowed_schema_preserves_encoding_and_reader_policy() {
    let (_temp, mut engine, repo) = fixture();
    let id = save(&mut engine, &repo.main_branch_id, b"schema bytes");
    let version = &engine.state.versions[&id];
    let slot = &engine.state.placements[&id].route_slot_ids[0];
    let directory = version_dir(engine.disk(slot).unwrap(), &version.history_tree_id, &id);
    let mut local: LocalPublication = read_json(&directory.join("publication-proof.json")).unwrap();
    let expected = assert_local_publication_encoding(&local);
    let mut raw: json_codec::Value = decode_strict(&expected).unwrap();
    raw["additional_local_field"] = json_codec::json!(true);
    let accepted: LocalPublication = decode_strict(&canonical_json(&raw).unwrap()).unwrap();
    assert_eq!(assert_local_publication_encoding(&accepted), expected);
    raw["candidate"]["additional_candidate_field"] = json_codec::json!(true);
    assert!(decode_strict::<LocalPublication>(&canonical_json(&raw).unwrap()).is_err());
    let duplicate = [b"{\"candidate\":null,".as_slice(), &expected[1..]].concat();
    assert!(decode_strict::<LocalPublication>(&duplicate).is_err());

    // Codec-only variants of a real proof: these are never persisted or treated
    // as valid receipts. Preserve original serde attributes and nested escaping.
    local.candidate.sequence = u64::MAX;
    local.publication.result.sequence = u64::MAX;
    local.publication.result.result = json_codec::json!({
        "\u{10000}": ["目录\n\"\\", null, true],
        "\u{e000}": "UTF-16 key order",
    });
    assert_local_publication_encoding(&local);
    crate::journal::set_candidate_catalog(
        &[],
        &mut local.candidate,
        std::collections::BTreeMap::new(),
    );
    local.candidate.previous_snapshot = None;
    assert_local_publication_encoding(&local);
}

#[test]
fn local_publication_borrowed_encoding_keeps_invalid_json_before_size_error() {
    let (_temp, mut engine, repo) = fixture();
    let id = save(&mut engine, &repo.main_branch_id, b"error boundary bytes");
    let version = &engine.state.versions[&id];
    let slot = &engine.state.placements[&id].route_slot_ids[0];
    let directory = version_dir(engine.disk(slot).unwrap(), &version.history_tree_id, &id);
    let mut local: LocalPublication = read_json(&directory.join("publication-proof.json")).unwrap();
    for invalid in [json_codec::json!(1.5), json_codec::json!(u64::MAX)] {
        local.publication.result.result = invalid;
        let view = LocalPublicationRef {
            candidate: &local.candidate,
            publication: &local.publication,
            commit_proof: &local.commit_proof,
            publication_receipts: &local.publication_receipts,
        };
        for maximum in [0, MAX_METADATA] {
            let old_error = r29_local_publication::encode(&local, maximum).unwrap_err();
            assert_ne!(old_error.code(), storage_contract::ErrorCode::ResourceLimit);
            for new_error in [
                encode_metadata(&view, maximum).unwrap_err(),
                encode_metadata(&local, maximum).unwrap_err(),
            ] {
                assert_eq!(new_error.code(), old_error.code());
                assert_eq!(new_error.to_string(), old_error.to_string());
                assert_eq!(
                    std::error::Error::source(&new_error).map(ToString::to_string),
                    std::error::Error::source(&old_error).map(ToString::to_string),
                );
            }
        }
    }
}

#[test]
fn recovery_rebuilds_each_local_publication_with_the_original_owned_bytes() {
    for corrupt in [false, true] {
        let (_temp, mut engine, repo) = fixture();
        let id = save(&mut engine, &repo.main_branch_id, b"recovery bytes");
        let version = engine.state.versions[&id].clone();
        let paths = engine.state.placements[&id]
            .route_slot_ids
            .iter()
            .map(|slot| {
                let directory =
                    version_dir(engine.disk(slot).unwrap(), &version.history_tree_id, &id);
                let path = directory.join("publication-proof.json");
                let local: LocalPublication = read_json(&path).unwrap();
                let expected = assert_local_publication_encoding(&local);
                assert_eq!(fs_bytes(&path), expected);
                (directory, path, expected)
            })
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 3);
        let root = engine.root.clone();
        drop(engine);
        for (_, path, _) in &paths {
            if corrupt {
                std::fs::write(path, b"corrupt local proof").unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
        }
        let recovered = Engine::open(&root).unwrap();
        for (directory, path, expected) in &paths {
            assert_eq!(fs_bytes(path), *expected);
            let local: LocalPublication = read_json(path).unwrap();
            assert_eq!(assert_local_publication_encoding(&local), *expected);
            verify_local_publication(directory, &version).unwrap();
        }
        drop(recovered);
        let _reopened = Engine::open(&root).unwrap();
        for (_, path, expected) in &paths {
            assert_eq!(fs_bytes(path), *expected);
        }
    }
}

#[test]
fn metadata_only_package_and_purge_tombstone_combinations_are_preflighted() {
    let (_temp, mut engine, repo) = fixture();
    let old = save(&mut engine, &repo.main_branch_id, b"old");
    save(&mut engine, &repo.main_branch_id, b"new");
    let plan = engine
        .create_purge_plan(&repo.history_tree_id, vec![old.clone()], "alice")
        .unwrap();
    engine
        .commit_plan(
            &plan.plan_id,
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let disk = engine.members()[0].clone();
    let scanned = scan(&disk).unwrap();
    let (record, _) = scanned.records.last().unwrap();
    let candidate: Candidate = load_object(&disk, "candidate", &record.candidate_hash).unwrap();
    assert!(candidate.versions.is_empty());
    let publication: Publication = load_object(&disk, "publication", &record.object_hash).unwrap();
    let parent = engine.root.join("package-output");
    private_dir(&parent).unwrap();
    let package = parent.join("tree");
    engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    let package_size = fs_bytes(&package.join("publication.json")).len();
    let seal: PublicationSeal =
        read_json(&publication_seal_path(&disk, &record.candidate_hash)).unwrap();
    assert_eq!(
        crate::history_package::package_proof_bytes(
            &candidate,
            &publication,
            &seal,
            crate::metadata_map::MapRoot::load(
                &candidate.history_roots,
                crate::metadata_map::ProofMeasure::Plain,
                |kind, hash| load_authenticated_object(&disk, kind, hash)
            )
            .unwrap()
            .maximum_inclusion_proof_bytes()
            .unwrap(),
            MAX_METADATA
        )
        .unwrap(),
        package_size
    );
    let tombstone = version_dir(&disk, &repo.history_tree_id, &old).join("purged.json");
    let tombstone_size = fs_bytes(&tombstone).len();
    let largest = preflight_publication(
        &engine,
        &candidate,
        &publication.result,
        &engine.state,
        None,
        MAX_METADATA,
    )
    .unwrap();
    assert_eq!(largest, package_size.max(tombstone_size));
    assert!(
        preflight_publication(
            &engine,
            &candidate,
            &publication.result,
            &engine.state,
            None,
            largest
        )
        .is_ok()
    );
    assert_eq!(
        preflight_publication(
            &engine,
            &candidate,
            &publication.result,
            &engine.state,
            None,
            largest - 1
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    assert!(package_size > canonical_json(&publication).unwrap().len());

    // Brute-force every actual eligible ID/path independently of NodeStats.
    // This codec fixture enlarges only the tombstone field, not pool history.
    let mut wide_state = engine.state().clone();
    for n in 0u32..600 {
        wide_state.content_states.insert(
            blake3::hash(&n.to_le_bytes()).to_hex().to_string(),
            if n % 3 == 0 {
                ContentState::Migrated
            } else {
                ContentState::Purged
            },
        );
    }
    let snapshot_hash = store_snapshot(std::slice::from_ref(&disk), &wide_state).unwrap();
    let snapshot: Snapshot = load_object(&disk, "snapshot", &snapshot_hash).unwrap();
    let pages = crate::journal::map_pages(&disk, &snapshot.maps["content_states"]);
    assert!(pages.len() > 5);
    let mut wide_candidate = candidate.clone();
    wide_candidate.snapshot_hash = snapshot_hash;
    let mut actual_largest = package_size;
    for (id, status) in &wide_state.content_states {
        if !matches!(status, ContentState::Purged | ContentState::Migrated) {
            continue;
        }
        let (value, content_state_proof) =
            crate::metadata_map::read_proof(&snapshot.maps["content_states"], id, |kind, hash| {
                load_authenticated_object(&disk, kind, hash)
            })
            .unwrap();
        assert_eq!(value.unwrap().as_str(), Some(status.as_str()));
        actual_largest = actual_largest.max(
            canonical_json(&ContentTombstone {
                version_id: id.clone(),
                state: *status,
                candidate: wide_candidate.clone(),
                publication: publication.clone(),
                publication_seal: seal.clone(),
                snapshot: snapshot.clone(),
                content_state_proof,
            })
            .unwrap()
            .len(),
        );
    }
    let bound = preflight_publication(
        &engine,
        &wide_candidate,
        &publication.result,
        &wide_state,
        None,
        MAX_METADATA,
    )
    .unwrap();
    assert_eq!(bound, actual_largest);
    assert_eq!(
        preflight_bound_publication(
            &engine,
            &wide_candidate,
            &publication.result,
            &wide_state,
            None,
            MAX_METADATA
        )
        .unwrap(),
        bound
    );
    assert!(
        preflight_publication(
            &engine,
            &wide_candidate,
            &publication.result,
            &wide_state,
            None,
            bound
        )
        .is_ok()
    );
    assert_eq!(
        preflight_publication(
            &engine,
            &wide_candidate,
            &publication.result,
            &wide_state,
            None,
            bound - 1
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceLimit
    );
    // Decided roots must read their exact stored paths; no reconstruction or
    // new canonical partition can stand in for a missing bound dependency.
    std::fs::remove_file(object_path(&disk, "state-page", &pages[0])).unwrap();
    assert!(
        preflight_bound_publication(
            &engine,
            &wide_candidate,
            &publication.result,
            &wide_state,
            None,
            MAX_METADATA
        )
        .is_err()
    );
    assert_eq!(
        preflight_publication(
            &engine,
            &wide_candidate,
            &publication.result,
            &wide_state,
            None,
            MAX_METADATA
        )
        .unwrap(),
        bound
    );
    assert!(!object_path(&disk, "state-page", &pages[0]).exists());
}

#[test]
fn snapshot_page_and_common_metadata_encoder_reject_one_byte_over_boundary() {
    let (_temp, engine, _repo) = fixture();
    let state = engine.state().clone();
    let mut high = 1usize;
    while store_snapshot_bounded(&[], &state, high).is_err() {
        high *= 2;
    }
    let mut low = 0usize;
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        if store_snapshot_bounded(&[], &state, mid).is_ok() {
            high = mid
        } else {
            low = mid
        }
    }
    assert!(store_snapshot_bounded(&[], &state, high).is_ok());
    assert_eq!(
        store_snapshot_bounded(&[], &state, high - 1)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    let object = json_codec::json!({"bytes":"full"});
    let size = canonical_json(&object).unwrap().len();
    assert_eq!(encode_metadata(&object, size).unwrap().len(), size);
    assert_eq!(
        encode_metadata(&object, size - 1).unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit
    );
}

#[test]
fn reused_content_pages_have_identical_exact_tombstone_budget_without_installing_fallback() {
    let (_temp, engine, _repo) = fixture();
    let members = engine.members();
    let scanned = scan(&members[0]).unwrap();
    let (record, _) = scanned.records.last().unwrap();
    let mut candidate: Candidate =
        load_object(&members[0], "candidate", &record.candidate_hash).unwrap();
    let mut result = engine.state.operations[&candidate.operation_id].clone();
    let mut before = engine.state().clone();
    for n in 0u32..600 {
        before.content_states.insert(
            blake3::hash(&n.to_le_bytes()).to_hex().to_string(),
            if n % 3 == 0 {
                storage_contract::ContentState::Migrated
            } else {
                storage_contract::ContentState::Purged
            },
        );
        before
            .file_notes
            .insert(format!("unrelated-{n:08}"), "note".into());
    }
    let prepared = prepare_snapshot(&before, MAX_METADATA).unwrap();
    prepared.store(&members, &before).unwrap();
    let mut after = before.clone();
    after.begin_tracking();
    after.sequence += 1;
    result.sequence = after.sequence;
    candidate.sequence = after.sequence;
    let fresh = prepare_snapshot(&after, MAX_METADATA).unwrap();
    let reused =
        prepare_snapshot_incremental(&before, &after, &prepared.reuse_basis(), MAX_METADATA)
            .unwrap();
    assert_eq!(fresh.hash, reused.hash);
    candidate.snapshot_hash = reused.hash.clone();
    let catalog = crate::history_package::prepare_history_manifests(&after, MAX_METADATA)
        .unwrap()
        .0;
    let budget = |snapshot: &PreparedSnapshot, maximum| {
        preflight_publication_prepared(
            &engine.config,
            &engine.disk_resources,
            &candidate,
            &result,
            &after,
            None,
            maximum,
            Some((snapshot, &catalog)),
        )
    };
    let largest = budget(&fresh, MAX_METADATA).unwrap();
    assert_eq!(budget(&reused, MAX_METADATA).unwrap(), largest);
    assert_eq!(budget(&reused, largest).unwrap(), largest);
    assert_eq!(
        budget(&reused, largest - 1).unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    let page_hashes =
        crate::journal::map_pages(&members[0], &prepared.descriptor.maps["content_states"]);
    assert!(page_hashes.len() > 1);
    for hash in &page_hashes {
        for disk in &members {
            std::fs::remove_file(object_path(disk, "state-page", hash)).unwrap();
        }
    }
    // An unrelated field is not decoded merely to size content tombstones.
    for hash in &crate::journal::map_pages(&members[0], &prepared.descriptor.maps["file_notes"]) {
        for disk in &members {
            std::fs::write(
                object_path(disk, "state-page", hash),
                b"unrelated corruption",
            )
            .unwrap();
        }
    }
    assert_eq!(budget(&reused, MAX_METADATA).unwrap(), largest);
    for disk in &members {
        assert!(!object_path(disk, "snapshot", &reused.hash).exists());
        for hash in &page_hashes {
            assert!(
                !object_path(disk, "state-page", hash).exists(),
                "preflight must not install reconstructed pages"
            );
        }
    }
}
