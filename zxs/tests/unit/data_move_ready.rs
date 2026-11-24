//! Disposable native READY recovery and refusal fixtures.
use super::super::ready::HeldReady;
use super::*;

const FIRST: Id = [0x77; 16];
const EMPTY: Id = [0x78; 16];
const RENAME: Id = [0x79; 16];

fn stage_entry(f: &Fixture, n: u64, op: Id) -> PathBuf {
    f.stage_path()
        .join(crate::container_store::entry_leaf(n, op))
}

fn no_selection(f: &Fixture) {
    let routes = f.system.path.join("container-routes");
    assert!(!routes.join(format!(".{}.incoming", hex(C))).exists());
    assert!(!f.target.root.path.join("data").join(hex(C)).exists());
    assert_eq!(
        fs::read(f.route()).unwrap(),
        crate::container_store::routes::encode(C, FROM).unwrap()
    );
    assert!(!f.journal().join("SWITCHED").exists());
    assert!(!f.journal().join("DONE").exists());
}

fn extend(f: &Fixture) {
    let owner = f.system.opaque_owner(c"container-owner").unwrap();
    let store = Store::open_roster(
        &f.system,
        vec![
            DataRoot {
                token: FROM,
                root: held(&f.source.root),
            },
            DataRoot {
                token: TO,
                root: held(&f.target.root),
            },
        ],
        owner,
    )
    .unwrap();
    store
        .save(
            Mutation {
                container: C,
                request: EMPTY,
            },
            Change::Save {
                target: Target::Branch {
                    name: "main".into(),
                    expected_head: [0x53; 16],
                },
                node: [0x54; 16],
                body: Body {
                    length: 0,
                    sha256: digest(b"").unwrap(),
                },
            },
            &b""[..],
            || Ok(()),
        )
        .unwrap();
    store
        .change(
            Mutation {
                container: C,
                request: RENAME,
            },
            Change::Rename {
                expected_sequence: 2,
                name: "after.bin".into(),
            },
            || Ok(()),
        )
        .unwrap();
}

#[test]
fn native_ready_copies_each_exact_receipt_and_raw_version_without_selecting() {
    let f = Fixture::new();
    extend(&f);
    let live = || f.state.require_named();
    let ready_id = {
        let selected = f.selected(&live);
        Pending::prepare(&selected)
            .unwrap()
            .copy_to_ready()
            .unwrap()
            .id()
    };
    let selected = f.selected(&live);
    assert_eq!(HeldReady::reopen(&selected).unwrap().id(), ready_id);
    let original = f.entry().parent().unwrap().to_path_buf();
    for (n, op) in [(1, FIRST), (2, EMPTY), (3, RENAME)] {
        let staged = stage_entry(&f, n, op);
        let from = original.join(staged.file_name().unwrap());
        assert_eq!(
            fs::read(from.join("receipt")).unwrap(),
            fs::read(staged.join("receipt")).unwrap()
        );
    }
    assert_eq!(
        fs::read(stage_entry(&f, 1, FIRST).join("payload")).unwrap(),
        RAW
    );
    assert_eq!(
        fs::read(stage_entry(&f, 2, EMPTY).join("payload")).unwrap(),
        b""
    );
    assert!(!stage_entry(&f, 3, RENAME).join("payload").exists());
    assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
    assert_eq!(
        fs::read(f.journal().join("new-route")).unwrap(),
        crate::container_store::routes::encode(C, TO).unwrap()
    );
    assert_eq!(
        fs::read(f.journal().join("target-proof")).unwrap().len(),
        144 + 128 * 3
    );
    assert!(f.marker(true).is_file());
    no_selection(&f);
}

#[test]
fn wrong_actual_source_digest_leaves_unproved_target_untouched_on_both_retries() {
    let f = Fixture::new();
    let wrong = vec![0; RAW.len()];
    fs::write(f.entry().join("payload"), &wrong).unwrap();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    let problem = Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .err()
        .unwrap();
    assert_eq!(problem.kind(), io::ErrorKind::InvalidData);
    assert!(problem.to_string().contains("SHA-256"));
    let raw_draft = stage_entry(&f, 1, FIRST).join("payload");
    assert_eq!(fs::read(&raw_draft).unwrap(), wrong);
    assert_eq!(fs::read(f.entry().join("payload")).unwrap(), wrong);
    let original = fs::File::open(&raw_draft).unwrap();
    let original_inode = std::os::unix::fs::MetadataExt::ino(&original.metadata().unwrap());
    let retry = Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .err()
        .unwrap();
    assert_eq!(retry.kind(), io::ErrorKind::InvalidData);
    assert!(retry.to_string().contains("SHA-256"));
    assert!(HeldReady::reopen(&selected).is_err());
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&fs::metadata(&raw_draft).unwrap()),
        original_inode
    );
    assert_eq!(fs::read(&raw_draft).unwrap(), wrong);
    assert_eq!(fs::read(f.entry().join("payload")).unwrap(), wrong);
    assert!(!f.journal().join("READY").exists());
    assert!(!f.journal().join("new-route").exists());
    no_selection(&f);
}

#[test]
fn last_body_liveness_loss_cannot_publish_even_a_ready_record() {
    let f = Fixture::new();
    let output = stage_entry(&f, 1, FIRST).join("payload");
    let observed = Cell::new(false);
    let live = || {
        f.state.require_named()?;
        if fs::metadata(&output).is_ok_and(|m| m.len() == RAW.len() as u64) {
            observed.set(true);
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    };
    let selected = f.selected(&live);
    let error = Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(observed.get());
    assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
    assert_eq!(fs::read(&output).unwrap(), RAW);
    assert!(!f.journal().join("READY").exists());
    assert!(!f.journal().join("target-proof").exists());
    no_selection(&f);
}

#[test]
fn same_bytes_in_a_foreign_source_or_target_child_fail_late_ready_proof() {
    for source in [true, false] {
        let f = Fixture::new();
        let leaf = if source {
            f.entry()
        } else {
            stage_entry(&f, 1, FIRST)
        };
        let named = leaf.join("payload");
        let backup = leaf
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("original-fixture-payload");
        let swap = Cell::new(true);
        let live = || {
            f.state.require_named()?;
            if f.journal().join("new-route").exists() && swap.replace(false) {
                fs::rename(&named, &backup)?;
                write_file(&named, RAW)?;
            }
            Ok(())
        };
        let selected = f.selected(&live);
        assert!(
            Pending::prepare(&selected)
                .unwrap()
                .copy_to_ready()
                .is_err()
        );
        assert!(!swap.get());
        assert_eq!(fs::read(&named).unwrap(), RAW);
        assert_eq!(fs::read(&backup).unwrap(), RAW);
        assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
        assert!(!f.journal().join("READY").exists());
        assert!(f.marker(true).is_file());
        no_selection(&f);
    }
}

#[test]
fn visible_ready_after_failed_parent_sync_requires_same_candidate_and_a_cold_verification() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let passed_cut = Cell::new(false);
    {
        let selected = f.selected(&live);
        let result = Pending::prepare(&selected)
            .unwrap()
            .copy_to_ready_with(|file| {
                let parent = f
                    .system
                    .child_directory(c"data-relocations")
                    .unwrap()
                    .unwrap();
                same_file(&parent, &parent.file, file).unwrap();
                assert!(f.journal().join("READY").is_file());
                passed_cut.set(true);
                Err(io::Error::other("fixture READY parent flush cut"))
            });
        assert!(result.is_err());
        assert!(passed_cut.get());
    }
    let before = fs::read(f.journal().join("READY")).unwrap();
    let candidate = f.journal().join("new-route");
    let raw = fs::read(&candidate).unwrap();
    let selected = f.selected(&live);
    let proved = HeldReady::reopen(&selected).unwrap().id();
    assert_eq!(&before[204..], &proved);

    let original = f.system.path.join("kept-original-candidate");
    fs::rename(&candidate, &original).unwrap();
    write_file(&candidate, &raw).unwrap();
    assert!(HeldReady::reopen(&selected).is_err());
    assert_eq!(fs::read(&candidate).unwrap(), raw);
    assert_eq!(fs::read(&original).unwrap(), raw);
    assert_eq!(fs::read(f.journal().join("READY")).unwrap(), before);
    assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
    assert_eq!(
        fs::read(stage_entry(&f, 1, FIRST).join("payload")).unwrap(),
        RAW
    );
    no_selection(&f);
}

#[test]
fn cold_ready_checks_actual_target_sha_even_with_matching_native_proof() {
    use super::super::super::super::Ready;
    use super::super::super::{
        proof::Side,
        source::{self, Member},
    };

    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();
    let raw = stage_entry(&f, 1, FIRST).join("payload");
    let wrong = vec![0; RAW.len()];
    fs::write(&raw, &wrong).unwrap();

    // Construct a consistent metadata fixture deliberately. Exact current
    // inode/length/stamps, immutable receipt bytes and checksums cannot
    // substitute the cold actual-body SHA and EOF check.
    let staged = selected
        .target_staging
        .child_directory(&selected.names.target_stage)
        .unwrap()
        .unwrap();
    let mut proof = source::walk(&staged, selected.scope, &live, |receipt| {
        Ok(Member::open(&staged, receipt)?.row)
    })
    .unwrap();
    proof
        .bind(Side::Target, selected.scope, TO, staged.entity.inode)
        .unwrap();
    let journal = f.journal();
    let intent = Intent::decode(&fs::read(journal.join("INTENT")).unwrap()).unwrap();
    let original = fs::read(journal.join("READY")).unwrap();
    let mut modeled = Ready::decode(&original, &intent).unwrap().value().clone();
    modeled.target_proof = digest(&proof.bytes).unwrap();
    fs::write(journal.join("target-proof"), &proof.bytes).unwrap();
    let modeled = modeled.encode(&intent).unwrap();
    fs::write(journal.join("READY"), modeled).unwrap();

    let error = HeldReady::reopen(&selected).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("SHA-256"));
    assert_eq!(fs::read(&raw).unwrap(), wrong);
    assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
    assert_eq!(fs::read(journal.join("READY")).unwrap(), modeled);
    no_selection(&f);
}

fn final_data(f: &Fixture) -> PathBuf {
    f.target.root.path.join("data").join(hex(C))
}

fn incoming_route(f: &Fixture) -> PathBuf {
    f.system
        .path
        .join("container-routes")
        .join(format!(".{}.incoming", hex(C)))
}

fn unchanged_authority(f: &Fixture) {
    assert_eq!(
        fs::read(f.route()).unwrap(),
        crate::container_store::routes::encode(C, FROM).unwrap()
    );
    assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
    assert!(f.marker(true).is_file());
    assert!(!f.journal().join("SWITCHED").exists());
    assert!(!f.journal().join("DONE").exists());
}

#[test]
fn both_arrivals_keep_exact_native_instances_and_leave_original_cr1_selected() {
    use std::os::unix::fs::MetadataExt;

    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    let ready = Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();
    let old_tree = fs::metadata(f.stage_path()).unwrap().ino();
    let private = f.journal().join("new-route");
    let paper = fs::metadata(&private).unwrap().ino();
    let paper_bytes = fs::read(&private).unwrap();
    let ready_id = ready.id();
    assert_eq!(ready.arrive().unwrap().id(), ready_id);
    assert_eq!(fs::metadata(final_data(&f)).unwrap().ino(), old_tree);
    let routes = incoming_route(&f);
    assert_eq!(fs::metadata(&routes).unwrap().ino(), paper);
    assert_eq!(fs::read(&routes).unwrap(), paper_bytes);
    assert_eq!(
        fs::read(
            final_data(&f)
                .join(f.entry().file_name().unwrap())
                .join("payload")
        )
        .unwrap(),
        RAW
    );
    assert!(!f.stage_path().exists());
    assert!(!private.exists());
    assert_eq!(
        HeldReady::reopen(&selected).unwrap().arrive().unwrap().id(),
        ready_id
    );
    unchanged_authority(&f);
}

#[test]
fn cold_after_target_arrival_is_the_only_legal_intermediate_position() {
    use std::os::unix::fs::MetadataExt;

    let f = Fixture::new();
    let arrived = final_data(&f);
    let block = Cell::new(true);
    let live = || {
        f.state.require_named()?;
        if block.get() && arrived.exists() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    };
    let selected = f.selected(&live);
    let ready = Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();
    let id = ready.id();
    let original_inode = fs::metadata(f.stage_path()).unwrap().ino();
    let result = ready.arrive();
    assert_eq!(result.err().unwrap().kind(), io::ErrorKind::TimedOut);
    assert_eq!(fs::metadata(&arrived).unwrap().ino(), original_inode);
    assert!(!f.stage_path().exists());
    assert!(f.journal().join("new-route").is_file());
    assert!(!incoming_route(&f).exists());
    unchanged_authority(&f);

    block.set(false);
    assert_eq!(
        HeldReady::reopen(&selected).unwrap().arrive().unwrap().id(),
        id
    );
    assert!(incoming_route(&f).is_file());
    assert!(!f.journal().join("new-route").exists());
    assert_eq!(fs::metadata(&arrived).unwrap().ino(), original_inode);
    unchanged_authority(&f);
}

#[test]
fn exact_both_arrived_can_settle_after_scoped_routes_parent_flush_failure() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    let ready = Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();
    let id = ready.id();
    let raw = fs::read(f.journal().join("new-route")).unwrap();
    let attempted = Cell::new(false);
    let result = ready.arrive_with(|file| {
        same_file(&selected.routes, &selected.routes.file, file).unwrap();
        assert!(final_data(&f).is_dir());
        assert!(incoming_route(&f).is_file());
        assert!(!f.journal().join("new-route").exists());
        assert!(!f.stage_path().exists());
        attempted.set(true);
        Err(io::Error::other("fixture exact routes parent flush cut"))
    });
    assert!(result.is_err());
    assert!(attempted.get());
    assert_eq!(fs::read(incoming_route(&f)).unwrap(), raw);
    assert_eq!(
        HeldReady::reopen(&selected).unwrap().arrive().unwrap().id(),
        id
    );
    unchanged_authority(&f);
}

#[test]
fn an_unexpected_target_or_out_of_order_candidate_is_not_adopted() {
    let f = Fixture::new();
    let foreign = final_data(&f);
    let insert = Cell::new(false);
    let live = || {
        f.state.require_named()?;
        if insert.replace(false) {
            ensure_dir(&foreign)?;
            write_file(&foreign.join("foreign"), b"retain without replacing")?;
        }
        Ok(())
    };
    let selected = f.selected(&live);
    let ready = Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();
    insert.set(true);
    assert!(ready.arrive().is_err());
    assert!(!insert.get());
    assert!(HeldReady::reopen(&selected).is_err());
    assert_eq!(
        fs::read(foreign.join("foreign")).unwrap(),
        b"retain without replacing"
    );
    assert_eq!(
        fs::read(stage_entry(&f, 1, FIRST).join("payload")).unwrap(),
        RAW
    );
    assert!(!incoming_route(&f).exists());
    unchanged_authority(&f);

    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();
    let private = f.journal().join("new-route");
    let raw = fs::read(&private).unwrap();
    fs::rename(&private, incoming_route(&f)).unwrap();
    assert!(HeldReady::reopen(&selected).is_err());
    assert_eq!(fs::read(incoming_route(&f)).unwrap(), raw);
    assert!(!private.exists());
    assert!(!final_data(&f).exists());
    assert!(f.stage_path().is_dir());
    unchanged_authority(&f);
}

#[test]
fn a_same_byte_replacement_old_cr1_blocks_routes_sync_after_candidate_arrival() {
    let f = Fixture::new();
    let original = f.route();
    let kept = f.system.path.join("kept-old-CR1");
    let raw = fs::read(&original).unwrap();
    let swap = Cell::new(true);
    let live = || {
        f.state.require_named()?;
        if incoming_route(&f).exists() && swap.replace(false) {
            fs::rename(&original, &kept)?;
            write_file(&original, &raw)?;
        }
        Ok(())
    };
    let selected = f.selected(&live);
    let ready = Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();
    let observed = Cell::new(false);
    let result = ready.arrive_with(|_| {
        observed.set(true);
        Ok(())
    });
    assert!(result.is_err());
    assert!(!swap.get());
    assert!(!observed.get());
    assert!(HeldReady::reopen(&selected).is_err());
    assert_eq!(fs::read(&kept).unwrap(), raw);
    assert_eq!(
        fs::read(incoming_route(&f)).unwrap(),
        crate::container_store::routes::encode(C, TO).unwrap()
    );
    assert!(final_data(&f).is_dir());
    unchanged_authority(&f);
}

#[test]
fn a_recreated_arrived_tree_is_not_the_ready_inode_even_with_same_bytes() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();

    let destination = final_data(&f);
    let kept = f.target.root.path.join("data").join("kept-ready-tree");
    fs::rename(f.stage_path(), &kept).unwrap();
    ensure_dir(&destination).unwrap();
    let leaf = crate::container_store::entry_leaf(1, FIRST);
    let duplicate = destination.join(&leaf);
    ensure_dir(&duplicate).unwrap();
    for name in ["receipt", "payload"] {
        fs::copy(kept.join(&leaf).join(name), duplicate.join(name)).unwrap();
    }
    let error = HeldReady::reopen(&selected).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("exact READY inode"));
    assert_eq!(fs::read(duplicate.join("payload")).unwrap(), RAW);
    assert_eq!(fs::read(kept.join(leaf).join("payload")).unwrap(), RAW);
    assert!(f.journal().join("new-route").exists());
    assert!(!incoming_route(&f).exists());
    unchanged_authority(&f);
}
