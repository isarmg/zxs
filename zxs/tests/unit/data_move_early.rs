//! Source fixtures for interruption before the I/G authoritative names.
use super::*;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

fn short(at: &CStr) -> &OsStr {
    OsStr::from_bytes(at.to_bytes())
}

#[test]
fn early_exact_prefixes_resume_without_publishing_an_incomplete_journal() {
    for cut in 0..6 {
        let f = Fixture::new();
        let live = || f.state.require_named();
        let (draft, original_birth, raw, proof) = {
            let selected = f.selected(&live);
            let route = Route::original(&selected).unwrap();
            let proof = selected.observe_source().unwrap();
            let parent = f.system.new_child_directory(MOVE_PARENT).unwrap();
            let work = early::journal_name(selected.scope).unwrap();
            let directory = parent.new_child_directory(&work).unwrap();
            let candidate = intend(&selected, &route, &proof, &directory).unwrap();
            let raw = candidate.encode().unwrap();
            if (1..5).contains(&cut) {
                directory
                    .create_opaque_record(&early::binding_name(&raw).unwrap())
                    .unwrap();
            }
            match cut {
                0 => {}
                1 => write_file(
                    &directory.path.join(short(early::PROOF_SPOOL)),
                    &proof.bytes[..25],
                )
                .unwrap(),
                _ => {
                    write_file(&directory.path.join(short(PROOF)), &proof.bytes).unwrap();
                    if cut == 3 {
                        write_file(&directory.path.join(short(early::INTENT_SPOOL)), &raw[..41])
                            .unwrap();
                    } else if cut >= 4 {
                        write_file(&directory.path.join(short(I)), &raw).unwrap();
                    }
                }
            }
            (directory.path, candidate.journal, raw, proof.bytes)
        };
        assert!(!f.journal().exists());
        assert!(!f.marker(false).exists());
        assert!(!f.marker(true).exists());
        f.intact();

        let cold = f.selected(&live);
        let pending = Pending::prepare(&cold).unwrap();
        assert_eq!(
            birth(&pending.journal.directory.file).unwrap(),
            original_birth
        );
        assert_eq!(fs::read(f.journal().join(short(I))).unwrap(), raw);
        assert_eq!(fs::read(f.journal().join(short(PROOF))).unwrap(), proof);
        assert!(!draft.exists());
        assert!(!f.marker(false).exists());
        assert!(!f.marker(true).exists());
        f.intact();
    }
}

#[test]
fn unknown_or_out_of_sequence_hidden_records_are_not_repaired() {
    for name in [early::PROOF_SPOOL, early::INTENT_SPOOL, c"other"] {
        let f = Fixture::new();
        let live = || f.state.require_named();
        let selected = f.selected(&live);
        let parent = f.system.new_child_directory(MOVE_PARENT).unwrap();
        let work = early::journal_name(selected.scope).unwrap();
        let directory = parent.new_child_directory(&work).unwrap();
        let route = Route::original(&selected).unwrap();
        let proof = selected.observe_source().unwrap();
        let candidate = intend(&selected, &route, &proof, &directory).unwrap();
        let binding = early::binding_name(&candidate.encode().unwrap()).unwrap();
        directory.create_opaque_record(&binding).unwrap();
        let changed = directory.path.join(short(name));
        write_file(&changed, b"outside early move proof").unwrap();
        assert!(Pending::prepare(&selected).is_err());
        assert_eq!(fs::read(changed).unwrap(), b"outside early move proof");
        assert!(!f.journal().exists());
        assert!(!f.marker(false).exists());
        assert!(!f.marker(true).exists());
        f.intact();
    }
}

#[test]
fn same_number_with_a_different_enrolled_target_cannot_adopt_source_only_evidence() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let (working_path, mark, proof_bytes) = {
        let selected = f.selected(&live);
        let route = Route::original(&selected).unwrap();
        let source = selected.observe_source().unwrap();
        let parent = f.system.new_child_directory(MOVE_PARENT).unwrap();
        let work = early::journal_name(selected.scope).unwrap();
        let directory = parent.new_child_directory(&work).unwrap();
        let raw = intend(&selected, &route, &source, &directory)
            .unwrap()
            .encode()
            .unwrap();
        let binding = early::binding_name(&raw).unwrap();
        directory
            .create_opaque_record(&binding)
            .unwrap()
            .sync_all()
            .unwrap();
        write_file(&directory.path.join(short(PROOF)), &source.bytes).unwrap();
        directory.file.sync_all().unwrap();
        (
            directory.path.join(short(PROOF)),
            directory.path.join(short(&binding)),
            source.bytes,
        )
    };
    let other = DataRoot {
        token: [0x4b; 32],
        root: private(&f.remote, "alternative-target"),
    };
    drop(other.root.opaque_owner(c"owner").unwrap());
    other.root.new_child_directory(c"data").unwrap();
    other.root.new_child_directory(c"staging").unwrap();
    let mut roots = f.roots();
    roots.data.1 = &other;
    let (scope, system, (from, _)) = f.scope();
    let same_number = (scope, system, (from, operand(&other.root).unwrap()));
    let selected = Selected::retain_names(roots, same_number, &live).unwrap();
    assert!(Pending::prepare(&selected).is_err());
    assert!(!f.journal().exists());
    assert_eq!(fs::read(working_path).unwrap(), proof_bytes);
    assert_eq!(fs::read(mark).unwrap(), [] as [u8; 0]);
    assert!(!f.marker(false).exists());
    assert!(!f.marker(true).exists());
    f.intact();
}

#[test]
fn birth_bound_inactive_spool_recovers_short_or_exact_but_not_foreign_bytes() {
    for (length, foreign) in [(0, false), (25, false), (GUARD_BYTES, false), (13, true)] {
        let f = Fixture::new();
        let live = || f.state.require_named();
        let (spool, before) = {
            let selected = f.selected(&live);
            let pending = Pending::prepare(&selected).unwrap();
            let name = &selected.names.guard_spool;
            let mut file = f.state.create_opaque_record(name).unwrap();
            let original_birth = birth(&file).unwrap();
            let raw = Guard {
                birth: original_birth,
            }
            .encode(&pending.journal.original)
            .unwrap();
            if foreign {
                std::io::Write::write_all(&mut file, b"foreign guard").unwrap();
            } else {
                std::io::Write::write_all(&mut file, &raw[..length]).unwrap();
            }
            (f.state.path.join(short(name)), original_birth)
        };
        assert!(!f.marker(false).exists());
        assert!(!f.marker(true).exists());
        f.intact();

        let cold = f.selected(&live);
        let pending = Pending::prepare(&cold).unwrap();
        if foreign {
            assert!(pending.arm().is_err());
            assert_eq!(fs::read(spool).unwrap(), b"foreign guard");
            assert!(!f.marker(true).exists());
        } else {
            let installed = pending.arm().unwrap();
            let after = birth(&installed.pin.file).unwrap();
            assert_eq!(installed.checked.value().birth, after);
            if length == GUARD_BYTES {
                assert_eq!(after, before);
            }
            assert_eq!(fs::read(f.marker(true)).unwrap(), installed.raw);
            assert!(!spool.exists());
        }
        assert!(!f.marker(false).exists());
        f.intact();
    }
}
