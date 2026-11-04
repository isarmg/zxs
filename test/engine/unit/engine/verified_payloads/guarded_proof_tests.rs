use super::*;

fn open_descriptors_for(path: &Path) -> usize {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| match std::fs::read_link(entry) {
            Ok(target) => target == path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => panic!("cannot inspect proof descriptor: {error}"),
        })
        .count()
}

#[test]
fn idle_payload_proof_closes_body_metadata_and_seal_without_another_validation() {
    let budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    let entry = &proof.entries[0];
    let binding = entry.disk.root.io_binding().unwrap();
    let body =
        crate::container_storage::content_path(&entry.directory, &candidate.versions[0].version)
            .unwrap();
    let seal = entry.disk.root.path().join("disk.json");
    let paths = std::iter::once(body)
        .chain(entry.metadata.iter().map(|(path, _)| path.clone()))
        .chain(std::iter::once(seal))
        .collect::<Vec<_>>();
    for path in &paths {
        assert_eq!(open_descriptors_for(path), 1, "{}", path.display());
    }
    assert!(binding.status().unwrap().ordinary > 0);
    let boundary = canonical_json(engine.state()).unwrap();

    drop(binding.begin_quiesce().unwrap().unwrap());
    for path in &paths {
        assert_eq!(open_descriptors_for(path), 0, "{}", path.display());
    }
    assert!(binding.status().unwrap().drained());
    assert_eq!(
        proof
            .validate(&engine.config, &candidate)
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), boundary);
    // Descriptor reservation is an optional cache budget, not active disk I/O.
    assert!(budget.held() > 0);
    drop(proof);
    assert_eq!(budget.held(), 0);
}

#[test]
fn uncached_payload_proof_still_revokes_its_retained_seal() {
    let budget = test_budget(0);
    let (_temp, engine, candidate) = fixture();
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    assert!(proof.full_revalidation);
    assert!(proof.entries.is_empty());
    let seal = &proof.disks[0];
    let binding = seal.disk.root.io_binding().unwrap();
    assert_eq!(open_descriptors_for(&seal.path), 1);
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert_eq!(open_descriptors_for(&seal.path), 0);
    assert!(binding.status().unwrap().drained());
    assert_eq!(
        proof
            .validate(&engine.config, &candidate)
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(budget.held(), 0);
}

#[test]
fn payload_proof_cannot_open_after_its_disk_gate_closes() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let disk = &engine.config.disks[0];
    let binding = engine
        .disk_resources
        .get(disk)
        .unwrap()
        .root
        .io_binding()
        .unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    let error = VerifiedPayloadSet::prepare(&engine, &candidate)
        .err()
        .expect("closed gate accepted a fresh proof");
    assert_eq!(error.code(), ErrorCode::DiskUnavailable);
    assert!(binding.status().unwrap().drained());
    assert_eq!(open_descriptors_for(&disk.root.join("disk.json")), 0);
}

#[test]
fn cancellation_after_last_replica_retires_proofs_in_cached_and_uncached_modes() {
    for capacity in [0, LEGACY_DESCRIPTOR_BUDGET] {
        let budget = test_budget(capacity);
        let (_temp, engine, candidate) = fixture();
        let baseline = engine
            .config
            .disks
            .iter()
            .map(|disk| {
                engine
                    .disk_resources
                    .get(disk)
                    .unwrap()
                    .root
                    .io_binding()
                    .unwrap()
                    .status()
                    .unwrap()
                    .ordinary
            })
            .collect::<Vec<_>>();
        let disk = &engine.config.disks[0];
        let binding = engine
            .disk_resources
            .get(disk)
            .unwrap()
            .root
            .io_binding()
            .unwrap()
            .clone();
        let during_validation = binding.clone();
        AFTER_REPLICA_VALIDATION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                drop(during_validation.begin_quiesce().unwrap().unwrap());
            }));
        });
        let error = VerifiedPayloadSet::prepare(&engine, &candidate)
            .err()
            .expect("cancelled preparation became a live proof");
        assert_eq!(error.code(), ErrorCode::DiskUnavailable);
        assert!(binding.status().unwrap().drained());
        assert_eq!(budget.held(), 0);
        for (disk, before) in engine.config.disks.iter().zip(baseline) {
            let status = engine
                .disk_resources
                .get(disk)
                .unwrap()
                .root
                .io_binding()
                .unwrap()
                .status()
                .unwrap();
            assert_eq!(
                status.ordinary,
                if status.quiescing { 0 } else { before },
                "failed proof does not retain additional I/O; active journals remain owned"
            );
        }
    }
}
