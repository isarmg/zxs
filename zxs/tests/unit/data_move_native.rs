//! Disposable native I/G fixtures, not a mounted product/worker qualification.
use super::super::{Names, Roots, Scope, digest, hex, operand};
use super::*;
use crate::{
    container_store::{
        Body, Change, DataRoot, Id, Mutation, Store, Target, ensure_dir, recycle::Recycle,
        recycle_receipt::RecycleRole, write_file,
    },
    test_support::PrivateTemp,
};
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
};

pub(super) const C: Id = [0x19; 16];
const O: Id = [0x2f; 16];
const POOL: Id = [0x39; 16];
const SYSTEM: Digest = [0x3a; 32];
const FROM: Digest = [0x3b; 32];
const TO: Digest = [0x3c; 32];
const R: Digest = [0x3d; 32];
pub(super) const RAW: &[u8] = b"original independent raw bytes";

pub(super) struct Fixture {
    pub(super) state: HeldDir,
    system: HeldDir,
    recycle: HeldDir,
    pub(super) source: DataRoot,
    pub(super) target: DataRoot,
    _local: PrivateTemp,
    remote: PrivateTemp,
}

fn private(root: &PrivateTemp, name: &str) -> HeldDir {
    let at = root.0.join(name);
    ensure_dir(&at).unwrap();
    HeldDir::trusted_storage(&at).unwrap()
}

fn held(other: &HeldDir) -> HeldDir {
    HeldDir::trusted_storage(&other.path).unwrap()
}

impl Fixture {
    pub(super) fn new() -> Self {
        let local = PrivateTemp::under(&std::env::temp_dir());
        let remote = PrivateTemp::under(Path::new("/dev/shm"));
        let state = private(&local, "state");
        let system = private(&local, "system");
        let recycle = private(&local, "recycle");
        let source = DataRoot {
            token: FROM,
            root: private(&local, "source"),
        };
        let target = DataRoot {
            token: TO,
            root: private(&remote, "target"),
        };
        assert_ne!(source.root.entity.device, target.root.entity.device);
        state.file.try_lock().unwrap();
        {
            let owner = system.opaque_owner(c"container-owner").unwrap();
            let store = Store::open_roster(
                &system,
                vec![
                    DataRoot {
                        token: FROM,
                        root: held(&source.root),
                    },
                    DataRoot {
                        token: TO,
                        root: held(&target.root),
                    },
                ],
                owner,
            )
            .unwrap();
            store.routes.reserve(C, FROM).unwrap();
            store
                .save(
                    Mutation {
                        container: C,
                        request: [0x77; 16],
                    },
                    Change::Save {
                        target: Target::FirstMain {
                            name: "original.bin".into(),
                        },
                        node: [0x53; 16],
                        body: Body {
                            length: RAW.len() as u64,
                            sha256: digest(RAW).unwrap(),
                        },
                    },
                    RAW,
                    || Ok(()),
                )
                .unwrap();
        }
        drop(
            Recycle::open(
                held(&system),
                held(&recycle),
                RecycleRole {
                    pool: POOL,
                    token: R,
                },
            )
            .unwrap(),
        );
        Self {
            state,
            system,
            recycle,
            source,
            target,
            _local: local,
            remote,
        }
    }

    fn roots(&self) -> Roots<'_> {
        Roots {
            state: &self.state,
            system: &self.system,
            recycle: &self.recycle,
            data: (&self.source, &self.target),
        }
    }

    fn scope(&self) -> (Scope, Digest, (Digest, Digest)) {
        (
            Scope {
                pool: POOL,
                container: C,
                operation: O,
            },
            SYSTEM,
            (
                operand(&self.source.root).unwrap(),
                operand(&self.target.root).unwrap(),
            ),
        )
    }

    pub(super) fn selected<'a>(&'a self, live: &'a dyn Fn() -> io::Result<()>) -> Selected<'a> {
        // Only the bootstrapped R fixture shares a filesystem with SOURCE.
        // These primitive tests consume its held names, not its physical
        // enrollment; retain() separately requires the real three-way split.
        Selected::retain_names(self.roots(), self.scope(), live)
            .ok()
            .unwrap()
    }

    fn names(&self) -> Names {
        Names::new(self.scope().0).unwrap()
    }

    fn journal(&self) -> PathBuf {
        self.system
            .path
            .join("data-relocations")
            .join(format!("{}-{}", hex(C), hex(O)))
    }

    fn stage_path(&self) -> PathBuf {
        self.target
            .root
            .path
            .join("staging")
            .join(format!("move-{}-{}", hex(C), hex(O)))
    }

    pub(super) fn marker(&self, active: bool) -> PathBuf {
        let prefix = if active {
            ".server-stage-move-"
        } else {
            ".move-guard-draft-"
        };
        self.state
            .path
            .join(format!("{prefix}{}-{}", hex(C), hex(O)))
    }

    fn route(&self) -> PathBuf {
        self.system.path.join("container-routes").join(hex(C))
    }

    fn entry(&self) -> PathBuf {
        self.source
            .root
            .path
            .join("data")
            .join(hex(C))
            .join(crate::container_store::entry_leaf(1, [0x77; 16]))
    }

    fn intact(&self) {
        assert_eq!(
            fs::read(self.route()).unwrap(),
            crate::container_store::routes::encode(C, FROM).unwrap()
        );
        assert_eq!(fs::read(self.entry().join("payload")).unwrap(), RAW);
        assert!(!self.target.root.path.join("data").join(hex(C)).exists());
        assert!(!self.stage_path().exists());
    }
}

#[test]
fn physical_role_admission_is_not_inferred_from_two_fixture_filesystems() {
    let f = Fixture::new();
    assert!(Selected::retain(f.roots(), f.scope(), &|| f.state.require_named()).is_err());
    assert!(!f.journal().exists());
    assert!(!f.marker(true).exists());
}

#[test]
fn exact_inactive_draft_resumes_with_the_same_live_inode_after_outer_denial() {
    let f = Fixture::new();
    let forbid_selection = Cell::new(false);
    let authorize = || {
        f.state.require_named()?;
        if forbid_selection.get() && f.marker(false).exists() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    };
    let intent = {
        let held = f.selected(&authorize);
        let plan = Pending::prepare(&held).unwrap();
        assert_eq!(fs::read(f.journal().join("INTENT")).unwrap().len(), 460);
        assert_eq!(
            fs::read(f.journal().join("source-proof")).unwrap().len(),
            144 + 128
        );
        assert!(!f.marker(true).exists());
        forbid_selection.set(true);
        assert!(plan.arm().is_err());
        assert!(f.marker(false).is_file());
        assert!(!f.marker(true).exists());
        plan.id()
    };
    let original = f.state.opaque_pin(&f.names().inactive).unwrap().unwrap();
    let inactive_birth = birth(&original.file).unwrap();
    let exact_draft = fs::read(f.marker(false)).unwrap();
    forbid_selection.set(false);
    let held = f.selected(&authorize);
    let recovered = Pending::prepare(&held).unwrap();
    assert_eq!(recovered.id(), intent);
    let armed = recovered.arm().unwrap();
    assert_eq!(birth(&armed.pin.file).unwrap(), inactive_birth);
    assert_eq!(fs::read(f.marker(true)).unwrap(), exact_draft);
    assert!(!f.marker(false).exists());
    assert_eq!(recovered.arm().unwrap().id(), armed.id());
    f.intact();
}

#[test]
fn visible_active_marker_with_failed_state_sync_is_not_success_and_must_settle() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    let plan = Pending::prepare(&selected).unwrap();
    let attempted_sync = Cell::new(false);
    assert!(
        plan.arm_with(|file| {
            same_file(&f.state, &f.state.file, file).unwrap();
            // Intentionally check the observed exact STATE descriptor, not just a synthetic failure.
            assert!(f.marker(true).is_file());
            assert!(!f.marker(false).exists());
            attempted_sync.set(true);
            Err(io::Error::other("fixture STATE flush cut"))
        })
        .is_err()
    );
    assert!(attempted_sync.get());
    let active = plan.active().unwrap();
    let counter = Cell::new(0);
    assert_eq!(
        plan.arm_with(|file| {
            counter.set(counter.get() + 1);
            file.sync_all()
        })
        .unwrap()
        .id(),
        active.id()
    );
    assert_eq!(counter.get(), 1);
    f.intact();
}

#[test]
fn a_same_byte_draft_copy_with_another_birth_and_an_unknown_active_are_not_adopted() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    let plan = Pending::prepare(&selected).unwrap();
    let name = f.names().inactive;
    let old = plan.stage_guard(&f.state, &name).unwrap();
    let saved = f.state.path.join("held_original_draft");
    fs::rename(f.marker(false), &saved).unwrap();
    fs::copy(&saved, f.marker(false)).unwrap();
    assert!(plan.arm().is_err());
    assert!(!f.marker(true).exists());
    assert_eq!(fs::read(f.marker(false)).unwrap(), old.raw);
    write_file(&f.marker(true), b"unrelated active evidence").unwrap();
    assert!(plan.arm().is_err());
    assert_eq!(
        fs::read(f.marker(true)).unwrap(),
        b"unrelated active evidence"
    );
    assert_eq!(fs::read(f.marker(false)).unwrap(), old.raw);
    f.intact();
}

#[test]
fn raw_prerequisites_veto_before_intent_without_adopting_private_stages() {
    for fault in 0..5 {
        let f = Fixture::new();
        let conflict = match fault {
            0 => f.stage_path(),
            1 => f.entry().join("foreign-raw-file"),
            2 => {
                let uploaded = f.system.path.join("upload-intents");
                ensure_dir(&uploaded).unwrap();
                uploaded.join(format!(".{}-0f.incoming", hex(C)))
            }
            3 => {
                let retired = f.system.path.join("retired-data");
                ensure_dir(&retired).unwrap();
                retired.join(format!(".{}.incoming", hex(TO)))
            }
            _ => f
                .recycle
                .path
                .join("containers")
                .join(format!("{}-{}", hex(C), hex(O))),
        };
        write_file(&conflict, b"untouched collision").unwrap();
        let live = || f.state.require_named();
        let selected = f.selected(&live);
        assert!(Pending::prepare(&selected).is_err());
        assert!(!f.journal().exists());
        assert!(!f.marker(true).exists());
        assert_eq!(fs::read(conflict).unwrap(), b"untouched collision");
        assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
    }
}

#[test]
fn an_incomplete_or_later_phase_journal_never_creates_an_active_guard() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    let prepared = Pending::prepare(&selected).unwrap();
    let intent = prepared.journal.raw;
    let saved = f.system.path.join("hold-intent");
    let named = f.journal().join("INTENT");
    fs::rename(&named, &saved).unwrap();
    assert!(Pending::prepare(&selected).is_err());
    assert!(!named.exists());
    assert!(!f.marker(true).exists());
    fs::rename(&saved, &named).unwrap();
    let later = f.journal().join("READY");
    write_file(&later, b"unknown later record").unwrap();
    assert!(Pending::prepare(&selected).is_err());
    assert!(prepared.arm().is_err());
    assert_eq!(fs::read(&named).unwrap(), intent);
    assert_eq!(fs::read(later).unwrap(), b"unknown later record");
    assert!(!f.marker(true).exists());
    f.intact();
}

#[test]
fn replacement_of_the_exact_cr1_precedes_any_durable_vacancy_flush() {
    let f = Fixture::new();
    let substitute = Cell::new(false);
    let original = f.route();
    let preserved = f.system.path.join("kept_cr1");
    let witness = fs::read(&original).unwrap();
    let outer = || {
        f.state.require_named()?;
        if substitute.replace(false) {
            fs::rename(&original, &preserved)?;
            write_file(&original, &witness)?;
        }
        Ok(())
    };
    let selected = f.selected(&outer);
    let bound = Route::original(&selected).unwrap();
    let flushed = Cell::new(false);
    substitute.set(true);
    assert!(
        bound
            .durable_vacancies_with(&selected, || {
                flushed.set(true);
                selected.vacancies(true)
            })
            .is_err()
    );
    assert!(!flushed.get());
    assert_eq!(fs::read(&original).unwrap(), witness);
    assert_eq!(fs::read(&preserved).unwrap(), witness);
    assert!(!f.journal().exists());
    assert!(!f.marker(true).exists());
    f.intact();
}

#[path = "data_move_early.rs"]
mod early_tests;

#[path = "data_move_ready.rs"]
mod ready_tests;

#[path = "data_move_switch.rs"]
mod switch_tests;

#[path = "data_move_retire.rs"]
mod retire_tests;
