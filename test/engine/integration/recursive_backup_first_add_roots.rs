//! Ordinary first Add: physical root feedback, not a terminal backup switch.
//! The original first-add BODY backfill test keeps its separate Current scope gap.
use crate::{files, operation};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use storage_contract::{DiskId, working_tree::RootBinding};
use storage_engine::{
    CommitRequest, CommitState, Config, Engine, ErrorCode, NamespaceOperation,
    PendingMemberDirectory, Repository, ValidatedConfig,
};

const BODY: &[u8] = b"ordinary A body survives a real first B addition";
const AREAS: [&str; 5] = [
    "common-data", "deleted-data", "zxs-data", "zxs-data/common-data", "zxs-data/deleted-data",
];

struct Fixture {
    temp: tempfile::TempDir,
    system: PathBuf,
    roots: [PathBuf; 2],
    request: Config,
    repository: Repository,
    version: String,
    engine: Engine,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700)).tempdir().unwrap();
        let system = temp.path().join("system");
        let roots = [temp.path().join("a"), temp.path().join("b")];
        let config = Config::development(["A", "B"].into_iter().zip(&roots)
            .map(|(name, path)| (name_policy::Name::parse(name).unwrap(), path.clone())))
            .unwrap();
        for path in std::iter::once(&system).chain(&roots) {
            fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        let mut initial = config.clone();
        initial.disks.truncate(1);
        let mut engine = Engine::initialize_held(
            &system, ValidatedConfig::try_from(initial).unwrap(),
            vec![fs::File::open(&roots[0]).unwrap()],
        ).unwrap();
        let repository = engine.create_disk_root("P01", "admin", operation()).unwrap();
        engine.file_namespace(&repository.repository_id, NamespaceOperation::Mkdir {
            path: name_policy::PortablePath::parse("docs").unwrap(),
        }, "admin", operation()).unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "docs/report", BODY).unwrap();
        assert_eq!(engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id, operation_id: operation(),
            principal: "admin".into(), expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token, message: "first Add source".into(),
            request_digest: None,
        }).unwrap().commit_state, CommitState::Published);
        let version = engine.namespace_tree(&repository.main_branch_id).unwrap()
            .get("docs/report").unwrap().version_id.clone();
        let mut request = engine.config().clone();
        request.disks.push(config.disks[1].clone());
        Self { temp, system, roots, request, repository, version, engine }
    }

    fn selected(&self) -> Vec<PendingMemberDirectory> {
        vec![PendingMemberDirectory {
            disk_id: self.request.disks[1].disk_id.clone(),
            directory: fs::File::open(&self.roots[1]).unwrap(),
        }]
    }

    fn begin(&mut self, op: storage_engine::OperationId) -> storage_engine::Result<()> {
        let selected = self.selected();
        self.engine.begin_membership_handoff_with_directories(
            self.request.clone(), "admin", op, selected,
        ).map(|status| assert!(status.all_members_fenced()))
    }

    fn activate(&mut self) {
        let archive = self.engine.build_managed_membership_control_archive().unwrap();
        let selected = self.selected();
        self.engine.enroll_pending_members(
            &archive.archive_path, &archive.root_hash, selected,
        ).unwrap();
        assert!(self.engine.resume_membership_joint_decision().unwrap()
            .all_certificates_synchronized());
        assert_eq!(self.engine.activate_pending_membership().unwrap().commit_state,
            CommitState::Published);
    }

    fn body(&self) {
        let mut content = self.engine.open_content(&self.version).unwrap();
        let mut bytes = Vec::new();
        content.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, BODY);
        assert_eq!(self.engine.namespace_tree(&self.repository.main_branch_id).unwrap()
            .get("docs/report").unwrap().version_id, self.version);
    }

    fn assert_binding(&self, op: storage_engine::OperationId, identities: &[(u64, u64)]) {
        let config = self.engine.config();
        assert_eq!(config.storage_roots.len(), self.request.storage_roots.len() + 1);
        for (disk, before) in &self.request.storage_roots {
            assert_eq!(config.storage_roots.get(disk), Some(before));
        }
        let disk = &self.request.disks[1];
        let record = &config.storage_roots[&disk.disk_id.parse::<DiskId>().unwrap()];
        assert_eq!(record.operation_id, op);
        assert_eq!(record.registration.disk_id.to_string(), disk.disk_id);
        assert_eq!(record.source_chain.len(), 1);
        assert_eq!(record.source_chain[0].disk_id.to_string(), self.request.disks[0].disk_id);
        assert_eq!((record.root_entity.device(), record.root_entity.inode()), identity(&disk.root));
        assert_eq!(binding_entities(record), identities);
        assert_eq!(area_identities(&disk.root), identities);
        let mut expected = self.request.clone();
        expected.storage_roots = config.storage_roots.clone();
        assert_eq!(storage_engine::canonical_json(config).unwrap(),
            storage_engine::canonical_json(&expected).unwrap());
        assert!(!self.engine.is_read_only());
        self.body();
    }
}

fn identity(path: &Path) -> (u64, u64) {
    let metadata = fs::metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}
fn area_identities(root: &Path) -> Vec<(u64, u64)> {
    AREAS.iter().map(|name| identity(&root.join(name))).collect()
}
fn binding_entities(binding: &RootBinding) -> Vec<(u64, u64)> {
    [binding.roots.common_data.entity, binding.roots.deleted_data.entity,
        binding.roots.zxs_data.entity, binding.roots.zxs_lifecycles.common_data.entity,
        binding.roots.zxs_lifecycles.deleted_data.entity]
        .into_iter().map(|entity| (entity.device(), entity.inode())).collect()
}
fn media(paths: &[&Path]) -> Vec<BTreeMap<PathBuf, Vec<u8>>> {
    paths.iter().map(|path| files(path)).collect()
}
fn copy_tree(source: &Path, target: &Path) {
    fs::DirBuilder::new().mode(0o700).create(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let to = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), to).unwrap();
        }
    }
}

#[test]
fn fresh_first_add_binds_five_areas_through_joint_cold_and_rejects_replacement() {
    let mut fixture = Fixture::new();
    let op = operation();
    assert_eq!(fs::read_dir(&fixture.roots[1]).unwrap().count(), 0);
    fixture.begin(op).unwrap();
    assert_eq!(fixture.engine.config().storage_roots, fixture.request.storage_roots);
    assert_eq!(fixture.engine.config().disks.len(), 1);
    assert!(fixture.engine.pending_membership_status().is_none());
    let identities = area_identities(&fixture.roots[1]);
    let archive = fixture.engine.build_managed_membership_control_archive().unwrap();
    // Same contents and pending reservation cannot substitute for the held inode.
    let parked = fixture.temp.path().join("b-original");
    fs::rename(&fixture.roots[1], &parked).unwrap();
    copy_tree(&parked, &fixture.roots[1]);
    let before = media(&[&fixture.system, &fixture.roots[0], &parked, &fixture.roots[1]]);
    let selected = fixture.selected();
    let error = fixture.engine.enroll_pending_members(
        &archive.archive_path, &archive.root_hash, selected,
    ).unwrap_err();
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
    assert_eq!(media(&[&fixture.system, &fixture.roots[0], &parked, &fixture.roots[1]]), before);
    assert!(fixture.engine.pending_membership_status().is_none());
    fs::remove_dir_all(&fixture.roots[1]).unwrap();
    fs::rename(&parked, &fixture.roots[1]).unwrap();
    fixture.activate();
    fixture.assert_binding(op, &identities);
    let Fixture { temp, system, roots, request, repository, version, engine } = fixture;
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    let fixture = Fixture { temp, system, roots, request, repository, version, engine };
    fixture.assert_binding(op, &identities);
    // The final five-area binding is enforced again on cold admission.
    let Fixture { temp, system, roots, request, repository, version, engine } = fixture;
    drop(engine);
    let common = roots[1].join("common-data");
    let parked = temp.path().join("b-common-original");
    fs::rename(&common, &parked).unwrap();
    copy_tree(&parked, &common);
    let before = media(&[&system, &roots[0], &roots[1], &parked]);
    assert!(Engine::open(&system).is_err());
    assert_eq!(media(&[&system, &roots[0], &roots[1], &parked]), before);
    fs::remove_dir_all(&common).unwrap();
    fs::rename(&parked, &common).unwrap();
    let engine = Engine::open(&system).unwrap();
    Fixture { temp, system, roots, request, repository, version, engine }
        .assert_binding(op, &identities);
}

#[test]
fn fresh_first_add_resumes_only_recorded_preparation_occurrences() {
    #[cfg(feature = "fault-injection")]
    let cuts = [None, Some("after_membership_layout_root_lock"),
        Some("after_membership_layout_unrecorded_stage"),
        Some("after_membership_layout_feedback"), Some("after_membership_layout_area_install_1"),
        Some("after_membership_layout_install")];
    #[cfg(not(feature = "fault-injection"))]
    let cuts: [Option<&str>; 1] = [None];
    for cut in cuts {
        let mut fixture = Fixture::new();
        let op = operation();
        if let Some(point) = cut {
            #[cfg(feature = "fault-injection")]
            fixture.engine.inject_fault_once(point);
            let error = fixture.begin(op).unwrap_err();
            assert_eq!(error.code(), ErrorCode::InjectedFailure, "{point}: {error}");
            assert_eq!(error.message().as_ref(), format!("interrupted at {point}").as_str());
            assert!(fixture.engine.membership_fence_status().is_none());
            assert!(fixture.engine.pending_membership_status().is_none());
            assert_eq!(fixture.engine.config().storage_roots, fixture.request.storage_roots);
        } else {
            fixture.begin(op).unwrap();
        }
        let stages = fs::read_dir(&fixture.roots[1]).unwrap().map(|entry| entry.unwrap().path())
            .filter(|path| path.file_name().unwrap().to_str().unwrap()
                .starts_with("membership-layout-")).collect::<Vec<_>>();
        let mut recorded = Vec::new();
        for area in AREAS {
            let final_path = fixture.roots[1].join(area);
            let path = if final_path.exists() {
                final_path
            } else if let Some(stage) = stages.first() {
                stage.join(area)
            } else {
                continue;
            };
            if path.exists() {
                recorded.push(identity(&path));
            }
        }
        let Fixture { temp, system, roots, request, repository, version, engine } = fixture;
        drop(engine);
        let engine = Engine::open(&system).unwrap();
        let mut fixture = Fixture { temp, system, roots, request, repository, version, engine };
        fixture.body();
        if cut == Some("after_membership_layout_unrecorded_stage") {
            let before = media(&[&fixture.system, &fixture.roots[0], &fixture.roots[1]]);
            let error = fixture.begin(op).unwrap_err();
            assert_eq!(error.code(), ErrorCode::RecoveryRequired);
            assert_eq!(media(&[&fixture.system, &fixture.roots[0], &fixture.roots[1]]), before);
            assert!(fixture.engine.membership_fence_status().is_none());
            assert_eq!(stages.len(), 1);
            continue;
        }
        if cut == Some("after_membership_layout_feedback") {
            assert_eq!(stages.len(), 1);
            // This is the real immutable feedback from the old member. Missing
            // and unknown wire fields cannot become a target creation proof.
            let receipt = fixture.roots[0].join("metadata/membership-fresh-root-preparations")
                .join(format!("{op}.json"));
            let original = fs::read(&receipt).unwrap();
            for missing in [true, false] {
                let mut value: json_codec::Value = json_codec::from_slice(&original).unwrap();
                let fields = value.as_object_mut().unwrap();
                if missing {
                    fields.remove("stage");
                } else {
                    fields.insert("unrecognized_area".into(), json_codec::Value::Bool(true));
                }
                fs::write(&receipt, json_codec::to_vec(&value).unwrap()).unwrap();
                let before = media(&[&fixture.system, &fixture.roots[0], &fixture.roots[1]]);
                assert_eq!(fixture.begin(op).unwrap_err().code(), ErrorCode::ObjectCorrupt);
                assert_eq!(media(&[&fixture.system, &fixture.roots[0], &fixture.roots[1]]),
                    before);
                assert!(fixture.engine.membership_fence_status().is_none());
                fs::write(&receipt, &original).unwrap();
            }
            let mut changed = fixture.request.clone();
            changed.disks[1].name = name_policy::Name::parse("Different B").unwrap();
            let before = media(&[&fixture.system, &fixture.roots[0], &fixture.roots[1]]);
            let selected = fixture.selected();
            assert_eq!(fixture.engine.begin_membership_handoff_with_directories(
                changed, "admin", op, selected,
            ).unwrap_err().code(), ErrorCode::IdempotencyKeyReused);
            assert_eq!(media(&[&fixture.system, &fixture.roots[0], &fixture.roots[1]]), before);
            assert!(fixture.engine.membership_fence_status().is_none());
            let common = stages[0].join("common-data");
            let parked = fixture.temp.path().join("recorded-common");
            fs::rename(&common, &parked).unwrap();
            copy_tree(&parked, &common);
            let before = media(&[&fixture.system, &fixture.roots[0], &fixture.roots[1], &parked]);
            assert!(fixture.begin(op).is_err());
            assert_eq!(media(&[&fixture.system, &fixture.roots[0], &fixture.roots[1], &parked]),
                before);
            assert!(fixture.engine.membership_fence_status().is_none());
            fs::remove_dir_all(&common).unwrap();
            fs::rename(&parked, &common).unwrap();
        }
        fixture.begin(op).unwrap();
        let identities = area_identities(&fixture.roots[1]);
        if !recorded.is_empty() {
            assert_eq!(recorded, identities);
        }
        fixture.activate();
        fixture.assert_binding(op, &identities);
        let Fixture { temp, system, roots, request, repository, version, engine } = fixture;
        drop(engine);
        let engine = Engine::open(&system).unwrap();
        Fixture { temp, system, roots, request, repository, version, engine }
            .assert_binding(op, &identities);
    }
}
