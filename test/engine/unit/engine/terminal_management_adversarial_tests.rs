//! Attacks start with real Engine publications and durable final-voter fences.
//! Rehashing a caller-controlled head/graph cannot replace the journal, lineage,
//! complete namespace mask or actual process-wide descriptor closure.
use super::*;
use crate::{
    CommitRequest, Config, RepositoryName, decode_strict,
    disk::{DiskHandle, MAX_METADATA},
    journal::{AuthenticatedMetadataObject, JournalFrameReader, RecordType},
    metadata_map::{Encoder, ProofMeasure, Reference},
    object_hash,
};
use json_codec::{Map, Value};
use rustix::fs::inotify;
use std::{collections::BTreeMap, fs, path::PathBuf, sync::mpsc, time::Duration};

struct Live {
    temp: tempfile::TempDir,
    config: Config,
    engine: Engine,
    operation: OperationId,
}

impl Live {
    fn new(voters: usize, data_only: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config = Config::development(
            (0..voters + data_only).map(|n| temp.path().join(format!("disk-{n}"))),
        )
        .unwrap();
        for disk in config.disks.iter_mut().skip(voters) {
            disk.metadata_member = false;
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("terminal-adversarial").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "retained.txt",
                b"real old namespace",
            )
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "keep immutable history".into(),
                request_digest: None,
            })
            .unwrap();
        Self {
            temp,
            config,
            engine,
            operation: OperationId::generate().unwrap(),
        }
    }

    fn prior_exit(&mut self, index: usize, action: MembershipExitAction) {
        let status = self
            .engine
            .begin_disk_exit(
                &self.config.disks[index].disk_id,
                action,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        assert!(status.original_members > 1);
        assert!(status.all_members_fenced());
        let archive = self
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        self.engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        self.engine.resume_membership_joint_decision().unwrap();
        self.engine.activate_pending_disk_exit().unwrap();
    }

    fn prepare(&mut self) -> TerminalDiskExit {
        let target = self
            .engine
            .config
            .metadata_members()
            .next()
            .unwrap()
            .disk_id
            .clone();
        assert_eq!(self.engine.config.metadata_members().len(), 1);
        let status = self
            .engine
            .begin_disk_exit(
                &target,
                MembershipExitAction::Deregister,
                "admin",
                self.operation,
            )
            .unwrap();
        assert_eq!((status.original_members, status.original_quorum), (1, 1));
        assert!(status.all_members_fenced());
        self.engine.prepare_terminal_disk_exit().unwrap()
    }

    fn stop(mut self, complete: bool) -> Saved {
        let mut exit = self.prepare();
        if complete {
            drop(exit.complete(&mut self.engine).unwrap());
        }
        drop(exit);
        drop(self.engine);
        Saved {
            temp: self.temp,
            config: self.config,
            operation: self.operation,
        }
    }
}

#[test]
fn terminal_preparation_preserves_prior_exit_guards_and_waits_for_its_own_tail() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut live = Live::new(2, 0);
        let first = live.config.disks[0].clone();
        let original = live.engine.config.clone();
        let mut old_seal = live.engine.disk_resources.get(&first).unwrap().seal.clone();
        live.prior_exit(0, action);
        assert!(live.engine.pending_disk_exit.is_some());
        assert_eq!(
            live.engine.disk_exit_status().unwrap().unwrap().phase,
            crate::DiskExitPhase::Published
        );
        let parked = live.temp.path().join("prior-exit-absent");
        fs::rename(&first.root, &parked).unwrap();
        let prior_bytes = files(&parked);
        // A different physical domain must still be excluded by the retained
        // logical guard, not merely by an old handle to this particular inode.
        old_seal.inode = old_seal.inode.checked_add(1).unwrap();
        let mut terminal = live.prepare();
        assert!(live.engine.pending_disk_exit.is_some());
        assert_eq!(
            live.engine.disk_exit_status().unwrap().unwrap().phase,
            crate::DiskExitPhase::Fenced
        );
        assert!(matches!(
            DiskIoBinding::new(&old_seal, &original),
            Err(error) if error.code() == ErrorCode::DiskUnavailable
        ));
        let tail = terminal.binding.admit().unwrap();
        assert_eq!(
            terminal.complete(&mut live.engine).err().unwrap().code(),
            ErrorCode::DiskUnavailable
        );
        let completion = live
            .temp
            .path()
            .join("system/terminal-management")
            .join(live.operation.to_string())
            .join("complete.json");
        assert!(!completion.exists());
        assert!(live.engine.pending_disk_exit.is_some());
        assert_eq!(files(&parked), prior_bytes);
        assert!(!first.root.exists());
        drop(tail);
        let management = terminal.complete(&mut live.engine).unwrap();
        assert_eq!(management.status().operation_id, live.operation);
        assert_eq!(management.status().active_metadata_members, 0);
        assert_eq!(management.status().original_metadata_members, 1);
        assert_eq!(management.status().original_metadata_quorum, 1);
        assert_eq!(management.registry().historical_topology().len(), 2);
        assert_eq!(
            management.registered_disks().len(),
            usize::from(action == MembershipExitAction::PlannedOffline)
        );
        assert!(live.engine.pending_disk_exit.is_some());
        assert!(matches!(
            DiskIoBinding::new(&old_seal, &original),
            Err(error) if error.code() == ErrorCode::DiskUnavailable
        ));
        assert_eq!(files(&parked), prior_bytes);
        assert!(!first.root.exists());
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn terminal_preparation_cannot_replace_an_activating_ordinary_exit() {
    let mut live = Live::new(2, 0);
    live.engine
        .begin_disk_exit(
            &live.config.disks[0].disk_id,
            MembershipExitAction::Deregister,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let archive = live
        .engine
        .build_managed_membership_control_archive()
        .unwrap();
    live.engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    live.engine.resume_membership_joint_decision().unwrap();
    live.engine.inject_fault_once("after_disk_exit_stop");
    assert!(live.engine.activate_pending_disk_exit().is_err());
    let status = live.engine.disk_exit_status().unwrap().unwrap();
    assert_eq!(status.phase, crate::DiskExitPhase::Activating);
    let before = files(live.temp.path());
    assert_eq!(
        live.engine
            .prepare_terminal_disk_exit()
            .err()
            .unwrap()
            .code(),
        ErrorCode::ConfigConflict
    );
    assert_eq!(live.engine.disk_exit_status().unwrap().unwrap(), status);
    assert_eq!(files(live.temp.path()), before);
    assert!(live.engine.pending_disk_exit.is_some());
}

struct Saved {
    temp: tempfile::TempDir,
    config: Config,
    operation: OperationId,
}

impl Saved {
    fn system(&self) -> PathBuf {
        self.temp.path().join("system")
    }
    fn directory(&self) -> PathBuf {
        self.system()
            .join("terminal-management")
            .join(self.operation.to_string())
    }
    fn archive(&self) -> PathBuf {
        self.system()
            .join("membership-control-archives")
            .join(self.operation.to_string())
    }
    fn authority(&self) -> Value {
        read_json(&self.directory().join("authority.json"))
    }
    fn reselect(&self, authority: &Value) {
        // Even the untrusted selector is updated to the correct typed hash.
        // A mere object checksum mismatch is not the rejection under test.
        let parsed: TerminalAuthority = json_codec::from_value(authority.clone()).unwrap();
        let hash = parsed.hash().unwrap();
        assert_eq!(
            hash,
            object_hash("terminal-management-authority", authority).unwrap()
        );
        write_json(&self.directory().join("authority.json"), authority);
        let path = self.system().join("terminal-management/head.json");
        let mut head = read_json(&path);
        head["authority_hash"] = hash.into();
        head["operation_id"] = self.operation.to_string().into();
        write_json(&path, &head);
    }
    fn assert_rejected_without_stop(&self) -> Error {
        let before = files(self.temp.path());
        let error = TerminalManagement::open(&self.system()).err().unwrap();
        assert_eq!(
            files(self.temp.path()),
            before,
            "rejection may not rewrite its evidence"
        );
        assert!(!self.directory().join("complete.json").exists());
        let store = open_store(&self.system()).unwrap();
        assert_eq!(
            store
                .reload(&self.config.pool_id)
                .unwrap()
                .stop_hints()
                .count(),
            0
        );
        error
    }
}

fn read_json(path: &Path) -> Value {
    decode_strict(&fs::read(path).unwrap()).unwrap()
}
fn write_json(path: &Path, value: &Value) {
    fs::write(path, crate::canonical_json(value).unwrap()).unwrap();
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}
fn object_path(archive: &Path, kind: &str, hash: &str) -> PathBuf {
    archive
        .join("objects")
        .join(kind)
        .join(&hash[..2])
        .join(format!("{hash}.json"))
}
fn write_object(archive: &Path, kind: &str, value: &Value) -> String {
    let hash = object_hash(kind, value).unwrap();
    let path = object_path(archive, kind, &hash);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_json(&path, value);
    hash
}
fn archive_map(archive: &Path, reference: &Value) -> Map<String, Value> {
    let reference: Reference = json_codec::from_value(reference.clone()).unwrap();
    crate::metadata_map::read_map(&reference, |kind, hash| {
        AuthenticatedMetadataObject::authenticate(
            kind,
            hash,
            read_json(&object_path(archive, kind, hash)),
        )
    })
    .unwrap()
}
fn write_archive_map(archive: &Path, values: Map<String, Value>) -> Value {
    let mut encoder = Encoder::new(MAX_METADATA);
    let reference = encoder
        .map_root(values, ProofMeasure::Plain)
        .unwrap()
        .reference()
        .clone();
    for object in encoder.finish() {
        let path = object_path(archive, object.kind(), &object.hash);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, object.bytes()).unwrap();
    }
    json_codec::to_value(reference).unwrap()
}
fn graph_map(authority: &Value, reference: &Value) -> Map<String, Value> {
    let reference: Reference = json_codec::from_value(reference.clone()).unwrap();
    crate::metadata_map::read_map(&reference, |kind, hash| {
        let object = &authority["objects"][hash];
        assert_eq!(object["kind"], kind);
        AuthenticatedMetadataObject::authenticate(kind, hash, object["value"].clone())
    })
    .unwrap()
}
fn insert_graph_map(authority: &mut Value, values: Map<String, Value>) -> Value {
    let mut encoder = Encoder::new(MAX_METADATA);
    let reference = encoder
        .map_root(values, ProofMeasure::Plain)
        .unwrap()
        .reference()
        .clone();
    for object in encoder.finish() {
        authority["objects"].as_object_mut().unwrap().insert(
            object.hash.clone(),
            json_codec::json!({ "kind": object.kind(), "value": decode_strict::<Value>(object.bytes()).unwrap() }),
        );
    }
    json_codec::to_value(reference).unwrap()
}
fn replace_strings(value: &mut Value, replacements: &BTreeMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(replacement) = replacements.get(text) {
                *text = replacement.clone();
            }
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| replace_strings(value, replacements)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| replace_strings(value, replacements)),
        _ => {}
    }
}
fn rehash_graph(authority: &mut Value) {
    // Re-address every affected dependency through the actual immutable DAG.
    // Old and new root commitments and mask references move together.
    loop {
        let replacements: BTreeMap<_, _> = authority["objects"]
            .as_object()
            .unwrap()
            .iter()
            .filter_map(|(hash, object)| {
                let next = object_hash(object["kind"].as_str().unwrap(), &object["value"]).unwrap();
                (next != *hash).then(|| (hash.clone(), next))
            })
            .collect();
        if replacements.is_empty() {
            return;
        }
        let objects = authority["objects"].as_object_mut().unwrap();
        for (old, new) in &replacements {
            let object = objects.remove(old).unwrap();
            if let Some(previous) = objects.insert(new.clone(), object.clone()) {
                assert_eq!(previous, object);
            }
        }
        replace_strings(authority, &replacements);
    }
}
fn registry_hash(authority: &Value, field: &str) -> String {
    authority[field]["root_hash"].as_str().unwrap().to_owned()
}
fn mask_hash(authority: &Value) -> String {
    let registry = registry_hash(authority, "registry");
    authority["objects"][registry]["value"]["pending_exit"]["effect"]["occurrence_mask_hash"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn terminal_rehashed_actor_and_operation_still_need_the_original_voters_fence() {
    for field in ["principal", "operation_id"] {
        let mut saved = Live::new(1, 0).stop(false);
        let mut authority = saved.authority();
        let old = authority["intent"][field].as_str().unwrap().to_owned();
        let replacement = if field == "principal" {
            "other-admin".to_owned()
        } else {
            let operation = OperationId::generate().unwrap();
            let old_directory = saved.directory();
            let old_archive = saved.archive();
            saved.operation = operation;
            fs::rename(old_directory, saved.directory()).unwrap();
            fs::rename(old_archive, saved.archive()).unwrap();
            operation.to_string()
        };
        authority["intent"][field] = replacement.clone().into();
        let old_transition =
            object_hash("membership-transition", &saved.authority()["intent"]).unwrap();
        let transition = write_object(
            &saved.archive(),
            "membership-transition",
            &authority["intent"],
        );
        replace_strings(
            &mut authority["objects"],
            &BTreeMap::from([(old, replacement), (old_transition, transition.clone())]),
        );
        rehash_graph(&mut authority);
        let mut archive = read_json(&saved.archive().join("archive-root.json"));
        archive["transition_hash"] = transition.clone().into();
        authority["archive_root"] = object_hash("membership-control-archive", &archive)
            .unwrap()
            .into();
        write_json(&saved.archive().join("archive-root.json"), &archive);
        let mut binding = read_json(&saved.archive().join("archive-binding.json"));
        binding["transition_hash"] = transition.into();
        write_json(&saved.archive().join("archive-binding.json"), &binding);
        saved.reselect(&authority);
        let error = saved.assert_rejected_without_stop();
        assert_eq!(error.code(), ErrorCode::LogFork, "{field}: {error}");
        assert!(
            error.to_string().contains("different membership fence"),
            "{field}: {error}"
        );
    }
}

#[test]
fn terminal_rehashed_registry_actor_and_operation_do_not_replace_certified_intent() {
    for field in ["principal", "operation_id"] {
        let saved = Live::new(1, 0).stop(false);
        let mut authority = saved.authority();
        let old = authority["intent"][field].as_str().unwrap().to_owned();
        let replacement = if field == "principal" {
            "other-admin".to_owned()
        } else {
            OperationId::generate().unwrap().to_string()
        };
        replace_strings(
            &mut authority["objects"],
            &BTreeMap::from([(old, replacement)]),
        );
        rehash_graph(&mut authority);
        saved.reselect(&authority);
        let error = saved.assert_rejected_without_stop();
        assert_eq!(
            error.code(),
            ErrorCode::PublicationInvalid,
            "{field}: {error}"
        );
        assert!(
            error.to_string().contains("certified exit intent"),
            "{field}: {error}"
        );
    }
}

#[test]
fn terminal_rehashed_source_registry_cannot_select_another_frozen_publication() {
    for field in ["snapshot_hash", "sequence", "topology_generation"] {
        let saved = Live::new(1, 0).stop(false);
        let mut authority = saved.authority();
        let hash = registry_hash(&authority, "source_registry");
        authority["objects"][hash]["value"]["source"][field] = if field == "snapshot_hash" {
            "a".repeat(64).into()
        } else {
            "999".into()
        };
        rehash_graph(&mut authority);
        saved.reselect(&authority);
        let error = saved.assert_rejected_without_stop();
        assert!(
            error.to_string().contains("authenticated frozen boundary"),
            "{field}: {error}"
        );
    }
}

#[test]
fn terminal_rehashed_complete_archive_still_requires_its_real_fence_frame() {
    let saved = Live::new(1, 0).stop(false);
    let mut authority = saved.authority();
    let archive = saved.archive();
    let mut root = read_json(&archive.join("archive-root.json"));
    let mut members = archive_map(&archive, &root["members"]);
    let disk = &saved.config.disks[0];
    let journal = archive
        .join("journals")
        .join(&disk.disk_id)
        .join(&disk.slot_id)
        .join(disk.binding_generation.to_string())
        .join("journal.bin");
    let bytes = fs::read(&journal).unwrap();
    let mut reader = JournalFrameReader::new(bytes.as_slice());
    let mut end = 0;
    let mut tail = None;
    while let Some((record, hash)) = reader.next_record().unwrap() {
        if record.record_type == RecordType::MembershipFence {
            break;
        }
        end = reader.valid_bytes;
        tail = Some(hash);
    }
    assert!(end > 0 && end < bytes.len() as u64);
    let truncated = &bytes[..end as usize];
    fs::write(&journal, truncated).unwrap();
    let member = members.get_mut(&disk.disk_id).unwrap();
    member["journal"]["bytes"] = end.to_string().into();
    member["journal"]["tail_frame_hash"] = tail.unwrap().into();
    member["journal"]["content_hash"] = blake3::hash(truncated).to_hex().to_string().into();
    root["members"] = write_archive_map(&archive, members);
    authority["archive_root"] = object_hash("membership-control-archive", &root)
        .unwrap()
        .into();
    write_json(&archive.join("archive-root.json"), &root);
    saved.reselect(&authority);
    let error = saved.assert_rejected_without_stop();
    assert_eq!(error.code(), ErrorCode::LogFork);
    assert!(
        error.to_string().contains("admitted original fence"),
        "{error}"
    );
}

#[test]
fn terminal_archive_cannot_replace_old_three_member_history_with_surviving_quorum() {
    for removed in [1, 2] {
        let mut live = Live::new(3, 0);
        live.prior_exit(0, MembershipExitAction::Deregister);
        live.prior_exit(1, MembershipExitAction::Deregister);
        let saved = live.stop(false);
        // The latest voter and all its copied publications still exist. The
        // first generation nevertheless requires each original own journal.
        for disk in saved.config.disks.iter().take(removed) {
            let journal = saved
                .archive()
                .join("journals")
                .join(&disk.disk_id)
                .join(&disk.slot_id)
                .join(disk.binding_generation.to_string())
                .join("journal.bin");
            assert!(journal.is_file());
            fs::remove_file(journal).unwrap();
        }
        let before = files(saved.temp.path());
        assert!(TerminalManagement::open(&saved.system()).is_err());
        assert_eq!(files(saved.temp.path()), before);
        assert!(!saved.directory().join("complete.json").exists());
        // The two earlier real exits remain; no third stop was minted.
        let store = open_store(&saved.system()).unwrap();
        assert_eq!(
            store
                .reload(&saved.config.pool_id)
                .unwrap()
                .stop_hints()
                .count(),
            2
        );
    }
}

#[test]
fn terminal_archive_requires_noncurrent_publication_and_permanent_receipt_closure() {
    for kind in ["candidate", "snapshot", "publication", "finalize"] {
        let saved = Live::new(1, 0).stop(false);
        let archive = saved.archive();
        let root = read_json(&archive.join("archive-root.json"));
        let seals = archive_map(&archive, &root["publication_seals"]);
        let boundary = saved.authority()["intent"]["boundary"]["candidate_hash"]
            .as_str()
            .unwrap()
            .to_owned();
        let (old, seal) = seals
            .iter()
            .find(|(candidate, _)| **candidate != boundary)
            .unwrap();
        let seal = read_json(&object_path(
            &archive,
            "membership-publication-seal",
            seal.as_str().unwrap(),
        ));
        let candidate = read_json(&object_path(&archive, "candidate", old));
        let publication = read_json(&object_path(
            &archive,
            "publication",
            seal["publication_hash"].as_str().unwrap(),
        ));
        let hash = match kind {
            "candidate" => old.clone(),
            "snapshot" => candidate["snapshot_hash"].as_str().unwrap().to_owned(),
            "publication" => seal["publication_hash"].as_str().unwrap().to_owned(),
            "finalize" => object_hash("finalize", &publication["finalize"]).unwrap(),
            _ => unreachable!(),
        };
        fs::remove_file(object_path(&archive, kind, &hash)).unwrap();
        saved.assert_rejected_without_stop();
    }
}

#[test]
fn terminal_rehashed_occurrence_mask_cannot_omit_a_real_frozen_scope() {
    let saved = Live::new(1, 0).stop(false);
    let mut authority = saved.authority();
    let hash = mask_hash(&authority);
    let mut scopes = graph_map(&authority, &authority["objects"][&hash]["value"]["scopes"]);
    let scope = scopes.keys().next().unwrap().to_owned();
    scopes.remove(&scope).unwrap();
    let reference = insert_graph_map(&mut authority, scopes);
    authority["objects"][hash]["value"]["scopes"] = reference;
    rehash_graph(&mut authority);
    saved.reselect(&authority);
    let error = saved.assert_rejected_without_stop();
    assert_eq!(error.code(), ErrorCode::PublicationInvalid);
    assert!(error.to_string().contains("occurrence mask"), "{error}");
}

#[test]
fn terminal_source_cannot_reenable_disabled_or_restore_detached_predecessor_registration() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut live = Live::new(2, 0);
        live.prior_exit(0, action);
        let saved = live.stop(false);
        let mut authority = saved.authority();
        let hash = registry_hash(&authority, "source_registry");
        let registry = authority["objects"][&hash]["value"].clone();
        let disk = &saved.config.disks[0];
        let mut registrations = graph_map(&authority, &registry["registrations"]);
        match action {
            MembershipExitAction::PlannedOffline => {
                assert_eq!(registrations[&disk.disk_id]["mode"], "disabled");
                registrations.get_mut(&disk.disk_id).unwrap()["mode"] = "enabled".into();
            }
            MembershipExitAction::Deregister => {
                assert!(!registrations.contains_key(&disk.disk_id));
                let topology = graph_map(&authority, &registry["historical_topology"]);
                registrations.insert(
                    disk.disk_id.clone(),
                    json_codec::json!({
                        "identity": topology[&disk.disk_id]["identity"], "mode": "enabled",
                    }),
                );
                let mut detached = graph_map(&authority, &registry["detached"]);
                detached.remove(&disk.disk_id).unwrap();
                let reference = insert_graph_map(&mut authority, detached);
                authority["objects"][&hash]["value"]["detached"] = reference;
            }
        }
        let reference = insert_graph_map(&mut authority, registrations);
        authority["objects"][hash]["value"]["registrations"] = reference;
        rehash_graph(&mut authority);
        saved.reselect(&authority);
        let before = files(saved.temp.path());
        let error = TerminalManagement::open(&saved.system()).err().unwrap();
        assert!(
            error.to_string().contains("exact predecessor"),
            "{action:?}: {error}"
        );
        assert_eq!(files(saved.temp.path()), before);
        assert!(!saved.directory().join("complete.json").exists());
    }
}

#[test]
fn terminal_mask_replay_does_not_resurrect_detached_namespace_from_its_historical_head() {
    let mut live = Live::new(2, 0);
    live.prior_exit(0, MembershipExitAction::Deregister);
    let saved = live.stop(false);
    let mut authority = saved.authority();
    let hash = mask_hash(&authority);
    let mut scopes = graph_map(&authority, &authority["objects"][&hash]["value"]["scopes"]);
    let scope = scopes
        .values_mut()
        .find(|scope| {
            scope["binding"]["scope"]["kind"] == "branch-head"
                && scope["binding"]["owner"]["disk_id"] == saved.config.disks[0].disk_id
        })
        .unwrap();
    assert_eq!(scope["binding"]["disposition"], "hidden");
    scope["binding"]["disposition"] = "registered-namespace".into();
    let reference = insert_graph_map(&mut authority, scopes);
    authority["objects"][hash]["value"]["scopes"] = reference;
    rehash_graph(&mut authority);
    saved.reselect(&authority);
    let before = files(saved.temp.path());
    let error = TerminalManagement::open(&saved.system()).err().unwrap();
    assert_eq!(error.code(), ErrorCode::PublicationInvalid);
    assert!(error.to_string().contains("occurrence mask"), "{error}");
    assert_eq!(files(saved.temp.path()), before);
}

fn watch_roots(config: &Config) -> rustix::fd::OwnedFd {
    let watch =
        inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK).unwrap();
    for disk in &config.disks {
        inotify::add_watch(
            &watch,
            &disk.root,
            inotify::WatchFlags::OPEN | inotify::WatchFlags::ACCESS,
        )
        .unwrap();
    }
    watch
}
fn assert_no_root_open(watch: &rustix::fd::OwnedFd) {
    let mut bytes = [0u8; 4096];
    assert_eq!(
        rustix::io::read(watch, &mut bytes),
        Err(rustix::io::Errno::WOULDBLOCK)
    );
}

fn open_descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| match fs::read_link(entry) {
            Ok(target) => target == path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => panic!("cannot inspect test descriptor: {error}"),
        })
        .count()
}

#[test]
fn terminal_missing_or_corrupt_selectors_never_probe_any_old_storage_root() {
    for damage in [
        "missing-terminal-head",
        "corrupt-terminal-head",
        "missing-locator-head",
        "corrupt-locator-record",
        "rolled-back-locator-head",
    ] {
        let saved = Live::new(1, 2).stop(true);
        let terminal_head = saved.system().join("terminal-management/head.json");
        let locator = saved.system().join("bootstrap-discovery");
        if damage == "corrupt-terminal-head" {
            fs::write(&terminal_head, b"{}").unwrap();
        } else {
            fs::remove_file(&terminal_head).unwrap();
        }
        match damage {
            "missing-locator-head" => fs::remove_file(locator.join("head.json")).unwrap(),
            "corrupt-locator-record" => {
                fs::write(locator.join("record-00000000000000000002.json"), b"{}").unwrap()
            }
            "rolled-back-locator-head" => {
                let record = read_json(&locator.join("record-00000000000000000002.json"));
                write_json(&locator.join("head.json"), &record["previous"]);
            }
            _ => {}
        }
        let watch = watch_roots(&saved.config);
        assert_eq!(
            Engine::open(&saved.system()).err().unwrap().code(),
            ErrorCode::RecoveryRequired,
            "{damage}"
        );
        assert_eq!(
            Engine::open_for_recovery(&saved.system())
                .err()
                .unwrap()
                .code(),
            ErrorCode::RecoveryRequired,
            "{damage}"
        );
        assert!(
            TerminalManagement::open(&saved.system()).is_err(),
            "{damage}"
        );
        let mut probes = 0;
        let store = open_store(&saved.system()).unwrap();
        if let Ok(locator) = store.reload(&saved.config.pool_id)
            && let Ok(selection) = locator.select(&saved.config)
        {
            selection
                .try_for_each(|_| {
                    probes += 1;
                    Ok::<_, ()>(())
                })
                .unwrap();
        }
        assert_eq!(probes, 0, "{damage}");
        assert_no_root_open(&watch);
        // Validate the actual kernel watch, rather than relying on an unused counter.
        drop(fs::File::open(&saved.config.disks[0].root).unwrap());
        let mut event = [0u8; 4096];
        assert!(rustix::io::read(&watch, &mut event).unwrap() > 0);
    }
}

#[test]
fn terminal_resume_source_cannot_create_a_missing_completion_or_ignore_resume_completion() {
    for record in ["resume.json", "resume-complete.json"] {
        let saved = Live::new(1, 0).stop(false);
        write_json(
            &saved.directory().join(record),
            &read_json(&saved.system().join("terminal-management/head.json")),
        );
        let before = files(saved.temp.path());
        let watch = watch_roots(&saved.config);
        assert!(TerminalManagement::open_for_resume(&saved.system()).is_err());
        assert_eq!(
            TerminalManagement::open(&saved.system())
                .err()
                .unwrap()
                .code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(
            Engine::open(&saved.system()).err().unwrap().code(),
            ErrorCode::RecoveryRequired
        );
        assert_no_root_open(&watch);
        assert_eq!(files(saved.temp.path()), before);
        assert!(!saved.directory().join("complete.json").exists());
        let store = open_store(&saved.system()).unwrap();
        assert_eq!(
            store
                .reload(&saved.config.pool_id)
                .unwrap()
                .stop_hints()
                .count(),
            0
        );
    }
}

#[test]
fn terminal_conflicting_completion_is_not_overwritten_by_matching_preparation() {
    for field in ["authority_hash", "operation_id", "pool_id"] {
        let saved = Live::new(1, 0).stop(false);
        let mut complete = read_json(&saved.system().join("terminal-management/head.json"));
        complete[field] = match field {
            "authority_hash" => "e".repeat(64),
            "operation_id" => OperationId::generate().unwrap().to_string(),
            "pool_id" => "pool_eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned(),
            _ => unreachable!(),
        }
        .into();
        let path = saved.directory().join("complete.json");
        write_json(&path, &complete);
        let original = fs::read(&path).unwrap();
        let watch = watch_roots(&saved.config);
        assert!(
            TerminalManagement::open(&saved.system()).is_err(),
            "{field}"
        );
        assert!(
            TerminalManagement::open_for_resume(&saved.system()).is_err(),
            "{field}"
        );
        assert_eq!(
            Engine::open(&saved.system()).err().unwrap().code(),
            ErrorCode::RecoveryRequired
        );
        assert_no_root_open(&watch);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn terminal_close_waits_for_a_second_physical_domain_real_file_descriptor() {
    let mut live = Live::new(1, 0);
    let mut disk = live.config.disks[0].clone();
    disk.root = live.temp.path().join("earlier-physical-domain");
    crate::disk::private_dir(&disk.root).unwrap();
    fs::write(disk.root.join("tail.bin"), b"live physical descriptor").unwrap();
    // Production admission derives a second actual directory identity for the
    // same logical binding. No Verified stop or Closed proof is constructed.
    let earlier = DiskHandle::open(&live.engine.config, &disk, false).unwrap();
    let binding = earlier.root.io_binding().unwrap().clone();
    let path = disk.root.join("tail.bin");
    let timeout = Duration::from_secs(30);
    std::thread::scope(|threads| {
        let (entered, entered_rx) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let root = &earlier.root;
        let path = &path;
        let reader = threads.spawn(move || {
            let mut first = true;
            root.read_bytes_observed(path, 64, &mut |_| {
                if first {
                    first = false;
                    entered.send(()).unwrap();
                    released.recv_timeout(timeout).unwrap();
                }
                Ok(())
            })
        });
        entered_rx.recv_timeout(timeout).unwrap();
        assert_eq!(open_descriptors(path), 1);
        let mut exit = live.prepare();
        let error = exit.complete(&mut live.engine).err().unwrap();
        assert_eq!(error.code(), ErrorCode::DiskUnavailable);
        assert!(
            error.to_string().contains("earlier logical I/O domain"),
            "{error}"
        );
        assert!(
            exit.binding.closed_proof().is_ok(),
            "current domain alone is closed"
        );
        let complete = live
            .temp
            .path()
            .join("system/terminal-management")
            .join(live.operation.to_string())
            .join("complete.json");
        assert!(!complete.exists());
        drop(binding.begin_quiesce().unwrap());
        assert!(binding.closed_proof().is_err());
        assert_eq!(
            open_descriptors(path),
            1,
            "the synchronous read tail still owns its actual fd"
        );
        assert_eq!(
            exit.complete(&mut live.engine).err().unwrap().code(),
            ErrorCode::DiskUnavailable
        );
        release.send(()).unwrap();
        assert_eq!(
            reader.join().unwrap().unwrap_err().code(),
            ErrorCode::DiskUnavailable,
            "the paused read is cancelled before returning bytes"
        );
        assert_eq!(open_descriptors(path), 0);
        assert!(binding.closed_proof().is_ok());
        let management = exit.complete(&mut live.engine).unwrap();
        assert!(complete.is_file());
        assert!(!management.status().writable);
        assert_eq!(
            management
                .registry()
                .registration(&management.status().target)
                .unwrap(),
            None
        );
        assert_eq!(
            DiskHandle::open(&live.engine.config, &disk, false)
                .err()
                .unwrap()
                .code(),
            ErrorCode::DiskUnavailable
        );
    });
}
