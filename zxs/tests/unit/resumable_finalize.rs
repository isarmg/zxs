use super::*;
use crate::container_store::Receipt;
use std::os::unix::fs::MetadataExt;

fn make_complete(lane: &Uploads<'_>, plan: &Intent) -> [u8; 32] {
    let result = lane.begin(plan, &LIVE).unwrap();
    let mut chunk = block(RAW);
    chunk.mutation = plan.mutation;
    chunk.upload = result.upload;
    assert_eq!(
        lane.put(chunk, &mut &RAW[..], &LIVE).unwrap().status,
        Status::Complete
    );
    result.upload
}

fn payload_for(f: &Fixture, upload: [u8; 32]) -> PathBuf {
    f.data
        .join("staging")
        .join(format!("u-{}", hex(upload)))
        .join("payload")
}

fn adopted_payload(f: &Fixture, receipt: &Receipt) -> PathBuf {
    f.data
        .join("data")
        .join(hex(receipt.mutation.container))
        .join(format!(
            "{:016x}-{}",
            receipt.sequence,
            hex(receipt.mutation.request)
        ))
        .join("payload")
}

#[test]
fn adopts_the_same_raw_inode_and_returns_only_the_exact_selected_receipt() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    let id = make_complete(&lane, &intent());
    let old = fs::metadata(payload_for(&f, id)).unwrap();
    assert_eq!(store.routes.get(OP.container).unwrap(), None);
    let committed = lane.commit(OP, id, &LIVE).unwrap();
    assert!(committed.retired.unwrap());
    let receipt = committed.receipt;
    assert_eq!(
        receipt,
        Receipt {
            mutation: OP,
            sequence: 1,
            change: change(&intent())
        }
    );
    let path = adopted_payload(&f, &receipt);
    let now = fs::metadata(&path).unwrap();
    assert_eq!((old.dev(), old.ino()), (now.dev(), now.ino()));
    assert_eq!(fs::read(&path).unwrap(), RAW);
    assert!(!f.staging().exists() && !f.control().exists());
    assert!(matches!(lane.commit(OP, id, &LIVE), Err(Failure::Conflict)));
    assert!(matches!(
        lane.status(OP, Some(id), &LIVE),
        Err(Failure::Conflict)
    ));
    assert_eq!(
        store.operation_receipt(OP).unwrap(),
        Some(OperationReceipt::Mutation(receipt))
    );
}

#[test]
fn correct_chunks_with_the_wrong_whole_digest_never_become_selected() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    let mut plan = intent();
    plan.body.sha256[0] ^= 1;
    let id = make_complete(&lane, &plan);
    let original = fs::read(f.control()).unwrap();
    assert!(matches!(
        lane.commit(OP, id, &LIVE),
        Err(Failure::InvalidBody)
    ));
    assert_eq!(fs::read(payload_for(&f, id)).unwrap(), RAW);
    assert_eq!(fs::read(f.control()).unwrap(), original);
    Fixture::assert_not_published(&store);
}

fn prepare_without_selection(f: &Fixture, sequence: u64) -> [u8; 32] {
    let store = f.store();
    let lane = f.lane(&store);
    let id = make_complete(&lane, &intent());
    let directory = lane.directory(false, &LIVE).unwrap().unwrap();
    let slot = journal::read(&directory, OP.container).unwrap().unwrap();
    let mut stage = material::Stage::open(&store.volumes[0], &slot.record).unwrap();
    stage.verify_whole(&intent().body, LIVE).unwrap();
    let receipt = Receipt {
        mutation: OP,
        sequence,
        change: change(&intent()),
    };
    // A crash after a fully synced provisional receipt but before publication.
    stage
        .prepare_receipt(&store, &receipt, &LIVE, &LIVE)
        .unwrap();
    Fixture::assert_not_published(&store);
    id
}

#[test]
fn cold_replay_reuses_only_an_exact_still_current_provisional_receipt() {
    for sequence in [1, 2] {
        let f = Fixture::new();
        let id = prepare_without_selection(&f, sequence);
        let provisional = f.staging().join("receipt");
        let bytes = fs::read(&provisional).unwrap();
        let stamp = fs::metadata(&provisional).unwrap();
        let store = f.store();
        let lane = f.lane(&store);
        if sequence == 1 {
            let receipt = lane.commit(OP, id, &LIVE).unwrap().receipt;
            let selected = adopted_payload(&f, &receipt).with_file_name("receipt");
            assert_eq!(fs::metadata(&selected).unwrap().ino(), stamp.ino());
            assert_eq!(fs::read(selected).unwrap(), bytes);
        } else {
            assert!(matches!(lane.commit(OP, id, &LIVE), Err(Failure::Conflict)));
            assert_eq!(fs::metadata(&provisional).unwrap().ino(), stamp.ino());
            assert_eq!(fs::read(provisional).unwrap(), bytes);
            assert_eq!(fs::read(payload_for(&f, id)).unwrap(), RAW);
            Fixture::assert_not_published(&store);
        }
    }
}

#[test]
fn a_new_branch_head_preserves_both_chosen_history_and_unpublished_raw() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    let first = Mutation {
        container: OP.container,
        request: [8; 16],
    };
    let initial = intent();
    store.save(first, change(&initial), RAW, LIVE).unwrap();
    let mut plan = initial.clone();
    plan.node = [9; 16];
    plan.target = Target::Branch {
        name: "main".into(),
        expected_head: initial.node,
    };
    let id = make_complete(&lane, &plan);
    let competing = Mutation {
        container: OP.container,
        request: [10; 16],
    };
    let mut other = plan.clone();
    other.node = [11; 16];
    let newer = store.save(competing, change(&other), RAW, LIVE).unwrap();
    assert!(matches!(lane.commit(OP, id, &LIVE), Err(Failure::Conflict)));
    assert_eq!(fs::read(payload_for(&f, id)).unwrap(), RAW);
    assert!(f.control().exists());
    assert!(store.operation_receipt(OP).unwrap().is_none());
    assert_eq!(
        store.operation_receipt(competing).unwrap(),
        Some(OperationReceipt::Mutation(newer))
    );
}

#[test]
fn existing_exact_r3_receipt_does_not_delete_the_still_named_upload() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    let id = lane.begin(&intent(), &LIVE).unwrap().upload;
    let exact = store.save(OP, change(&intent()), RAW, LIVE).unwrap();
    let replay = lane.commit(OP, id, &LIVE).unwrap();
    assert_eq!(replay.receipt, exact);
    assert!(!replay.retired.unwrap());
    assert!(f.control().exists());
    assert!(payload_for(&f, id).exists());
}

#[test]
fn post_move_confirmation_requires_the_original_members_at_the_final_name() {
    let f = Fixture::new();
    let id = prepare_without_selection(&f, 1);
    let store = f.store();
    let lane = f.lane(&store);
    let directory = lane.directory(false, &LIVE).unwrap().unwrap();
    let slot = journal::read(&directory, OP.container).unwrap().unwrap();
    let stage = material::Stage::open(&store.volumes[0], &slot.record).unwrap();
    stage.verify_whole(&intent().body, LIVE).unwrap();
    let parent = store.volumes[0].contents();
    stage
        .choose(parent, c"fixture-generation", &LIVE, LIVE)
        .unwrap();
    parent.file.sync_all().unwrap();
    store.volumes[0].sync_staging().unwrap();
    stage.confirm_moved(parent, c"fixture-generation").unwrap();
    assert!(!payload_for(&f, id).exists());
    let selected = parent.path.join("fixture-generation").join("payload");
    let held = f.system.path.join("held-raw");
    fs::rename(&selected, &held).unwrap();
    write_file(&selected, RAW).unwrap();
    assert!(stage.confirm_moved(parent, c"fixture-generation").is_err());
    assert_eq!(fs::read(held).unwrap(), RAW);
    assert_eq!(fs::read(selected).unwrap(), RAW);
    assert!(f.control().exists());
}

#[test]
fn displacement_at_the_late_first_cut_leaves_no_authoritative_route_or_directory() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    let id = make_complete(&lane, &intent());
    let staged = payload_for(&f, id);
    let original = f.data.join("fixture-original");
    let changed = Cell::new(false);
    let authorize = || {
        // Simulate a name becoming replaced after the slow global name
        // decision but before first CR1's own final native publication.
        if matches!(
            store.names.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ) && !changed.replace(true)
        {
            fs::rename(&staged, &original)?;
            write_file(&staged, b"foreign")?;
        }
        Ok(())
    };
    assert!(matches!(
        lane.commit(OP, id, &authorize),
        Err(Failure::Io(_))
    ));
    assert!(changed.get());
    assert_eq!(fs::read(original).unwrap(), RAW);
    assert_eq!(fs::read(staged).unwrap(), b"foreign");
    assert!(f.control().exists());
    Fixture::assert_not_published(&store);
    assert!(
        store.volumes[0]
            .held_container(OP.container)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_prior_reservation_does_not_authorize_first_directory_creation_after_revocation() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    let id = make_complete(&lane, &intent());
    store.routes.reserve(OP.container, TOKEN).unwrap();
    let authorize = || {
        if matches!(
            store.names.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    };
    assert!(matches!(
        lane.commit(OP, id, &authorize),
        Err(Failure::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied
    ));
    assert_eq!(store.routes.get(OP.container).unwrap(), Some(TOKEN));
    assert!(
        store.volumes[0]
            .held_container(OP.container)
            .unwrap()
            .is_none()
    );
    assert!(store.operation_receipt(OP).unwrap().is_none());
    assert_eq!(fs::read(payload_for(&f, id)).unwrap(), RAW);
    assert!(f.control().exists());
}
