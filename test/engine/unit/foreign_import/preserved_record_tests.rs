use super::*;

#[test]
fn preserved_record_counts_canonical_utf8_and_escaping_at_the_exact_64_kib_boundary() {
    // A JSON string adds its two quotes; the embedded quote requires an escape.
    let at_limit = format!("{}\u{00e9}\"", "x".repeat(MAX_RECORD_BYTES - 6));
    let (exact, commitment) = encode(&at_limit).unwrap();
    assert_eq!(exact.len(), MAX_RECORD_BYTES);
    assert_eq!(
        commitment.0.to_string(),
        object_hash("foreign-preserved-chain-intent", &at_limit).unwrap()
    );
    assert_ne!(
        commitment.0.to_string(),
        blake3::hash(&exact).to_hex().to_string()
    );
    let above = format!("{at_limit}x");
    assert!(matches!(encode(&above), Err(error) if error.code() == ErrorCode::ResourceLimit));
}

fn fresh(root: &Path, prefix: &str, count: usize) -> Config {
    Config::development((0..count).map(|index| {
        let name = format!("{prefix}-{index}");
        (name_policy::Name::parse(&name).unwrap(), root.join(&name))
    })).unwrap()
}

fn save_source(engine: &mut Engine) {
    let repository = engine.create_repository(
        &RepositoryName::parse("source").unwrap(), "P01", 4, "admin",
        OperationId::generate().unwrap(),
    ).unwrap();
    let working = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&working.workspace_id, "read.txt", b"complete four-root FULL").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: working.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: working.base_head,
        fencing_token: working.fencing_token,
        message: String::new(),
        request_digest: None,
    }).unwrap();
}

#[test]
fn preserved_four_real_levels_bind_each_native_scope_and_refuse_a_changed_original_without_fencing() {
    use std::fs::{self, File};
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut foreign = Engine::initialize(
        &temp.path().join("source-system"), fresh(temp.path(), "original", 4),
    ).unwrap();
    save_source(&mut foreign);
    let high = foreign.config().disks[3].root.clone();
    drop(foreign);
    let source = ForeignImportPreview::inspect(
        File::open(&high).unwrap(), high.clone(), ForeignImportLimits::default(),
    ).unwrap();
    let operation = OperationId::generate().unwrap();
    let chain: ForeignEvidenceDigest = "b".repeat(64).parse().unwrap();
    let mut new_chain = fresh(temp.path(), "offered", 4).disks;
    for (index, disk) in new_chain.iter_mut().enumerate() {
        disk.slot_id = format!("F{:02}", index + 1);
        disk.upstream_slot_id = (index != 0).then(|| format!("F{index:02}"));
        if index == 3 {
            disk.root = high.clone();
            continue;
        }
        fs::create_dir(&disk.root).unwrap();
        fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
        let path = disk.root.join(layout::FOREIGN_CHAIN_CAPSULE.as_path());
        let plan = ForeignMigrationPlan::chain_material(&source, operation, &chain, disk.level)
            .unwrap();
        let digest = *plan.digest();
        let mut native = plan.begin(&path).unwrap();
        native.copy_source(&source).unwrap();
        native.seal().unwrap();
        native.publish(None).unwrap();
        let lower = ForeignMigrationTransaction::open_published_in(
            File::open(&disk.root).unwrap(), &path, &operation, &digest,
        ).unwrap();
        lower.prepare_original_databases().unwrap();
    }
    let system = temp.path().join("current-system");
    let mut current = Engine::initialize(&system, fresh(temp.path(), "existing", 2)).unwrap();
    let mut target = current.config().clone();
    target.disks.extend(new_chain.iter().cloned());
    // The original active area set is complete for the old roots; the four
    // newly offered physical directories cannot be converted into active ones.
    assert!(ValidatedConfig::try_from(target.clone()).is_err());
    let request = || ForeignIdentityTargetRequest {
        proposed_configuration: target.clone(),
        operation_id: operation,
        request_digest: "a".repeat(64).parse().unwrap(),
        chain_digest: chain,
        registration_proposal_digest: "c".repeat(64).parse().unwrap(),
    };
    let directories = || new_chain.iter().map(|disk| PendingMemberDirectory {
        disk_id: disk.disk_id.clone(),
        directory: File::open(&disk.root).unwrap(),
    }).collect();
    let observe = |engine: &Engine, actor| {
        engine.observe_preserved_foreign_handoff(request(), &source, actor, directories())
    };
    let verified_empty = observe(&current, "admin").unwrap();
    let empty = storage_contract::strict_json(&verified_empty.bytes).unwrap();
    assert!(empty["current_snapshot_hash"].is_null());
    assert_eq!(empty["current_sequence"], "0");
    let schema = empty.as_object().unwrap();
    assert_eq!(schema.len(), 14);
    assert_eq!(empty["format"], "foreign-preserved-chain-intent");
    let roots = empty["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 4);
    for (index, (row, disk)) in roots.iter().zip(&new_chain).enumerate() {
        assert_eq!(row["level"], index + 1);
        assert_eq!(row["registration"]["disk_id"], disk.disk_id);
        assert_eq!(row["path"], disk.root.to_str().unwrap());
        assert_eq!(row["material"]["kind"], if index == 3 { "source" } else { "lower" });
    }
    assert!(roots[0]["upstream_slot_id"].is_null());
    assert_eq!(roots[3]["material"]["original_registration"]["pool_id"], empty["original"]["pool_id"]);
    assert_ne!(roots[0]["material"]["capsule_plan_digest"], roots[1]["material"]["capsule_plan_digest"]);
    // One real local publication now selects the exact non-null current
    // snapshot, including both voters' actual independently admitted frames.
    current.create_repository(&RepositoryName::parse("current").unwrap(), "P01", 1,
        "admin", OperationId::generate().unwrap()).unwrap();
    let observed = observe(&current, "admin").unwrap();
    assert_ne!(verified_empty.digest.0, observed.digest.0);
    let record = storage_contract::strict_json(&observed.bytes).unwrap();
    assert_eq!(record["current_snapshot_hash"], current.snapshot.as_deref().unwrap());
    assert_eq!(record["current_state_hash"],
        object_hash("foreign-identity-current-state", current.state()).unwrap());
    let held = current.state.clone();
    std::sync::Arc::make_mut(&mut current.state).topology_generation += 1;
    assert!(source.preserved_current_snapshot(&current).is_err());
    current.state = held;
    // Seed only test-private SYSTEM with the exact observed bytes. The real
    // complete entry must never perform this write from untrusted digest input.
    let dir = system.join(layout::FOREIGN_ENROLLMENT.as_path());
    assert!(!dir.exists());
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join(layout::foreign_enrollment(operation).as_path());
    fs::write(&path, &observed.bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(observe(&current, "admin").unwrap().digest.0, observed.digest.0);
    assert!(matches!(observe(&current, "other"), Err(error)
        if error.code() == ErrorCode::IdempotencyKeyReused));
    let original = fs::read(&path).unwrap();
    let hidden = temp.path().join("kept-lower-two");
    fs::rename(&new_chain[1].root, &hidden).unwrap();
    fs::create_dir(&new_chain[1].root).unwrap();
    fs::set_permissions(&new_chain[1].root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(observe(&current, "admin").is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read_dir(&new_chain[1].root).unwrap().count(), 0);
    assert!(current.membership_fence_status().is_none());
}
