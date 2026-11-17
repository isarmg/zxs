use super::{BoundDirectoryRequest, PendingDirectoryDrafts, ReconstructionError};
use crate::persist::directory_draft_commit::{
    self as codec, ConstructionDescription, DecisionDescription, DecisionFormat,
    DescriptionError, ExpectedContent, FileOutcome, SelectedOutcome,
};
use crate::working_tree::snapshot::{SnapshotEntryContent, SnapshotFile};
use crate::Engine;
use std::sync::Arc;
use storage_contract::file_draft::DraftCommand;
use storage_contract::working_tree::SnapshotNodeId;
use storage_contract::{DiskId, FileNodeId};

#[path = "fixture.rs"]
mod fixture;
use fixture::{Fixture, inventory, selection, write_draft};

const DESCRIPTION_BYTES: usize = 1024 * 1024;
const NEW_FIRST_BASE: &[u8] = b"real newer FILE baseline than the old directory snapshot";

fn retained(
    engine: &Engine, request: &codec::RequestDescription,
) -> ConstructionDescription {
    let files = request.selected.iter().map(|selection| {
        let ExpectedContent::Born { base, .. } = selection.content else {
            panic!("fixture uses actual already-published Born files");
        };
        let version = &engine.state().versions[&base.version_id().to_string()];
        SelectedOutcome {
            target: selection.target.clone(),
            outcome: FileOutcome::Retained {
                base,
                material: SnapshotFile {
                    file_id: selection.target.file,
                    version_id: base.version_id(),
                    size: version.logical_size,
                    content_hash: version.content_hash,
                },
            },
        }
    }).collect();
    ConstructionDescription {
        request: request.clone(),
        // This is an uncommitted proposed directory allocation, not a prepared
        // node or a fabricated FILE Version. No authority can be built from it.
        node: SnapshotNodeId::generate().unwrap(),
        files,
    }
}

#[test]
fn strict_descriptions_bind_real_selection_and_reconstruct_without_publication() {
    let (fixture, mut engine) = Fixture::new();
    let original_directory = inventory(&fixture.snapshot_path());
    let initial_base = engine.file_draft_base(&fixture.targets[0]).unwrap();
    let principal = "admin".parse().unwrap();
    // Setup only: create a real independently Published newer FILE baseline.
    // The directory is still its original old node. No mixed batch is simulated.
    write_draft(&mut engine, &fixture.targets[0], NEW_FIRST_BASE);
    let commit = DraftCommand::Commit {
        target: fixture.targets[0].clone(),
        expected: engine.file_working_state(&fixture.targets[0]).unwrap().revision(),
        base: initial_base,
        message: "real newer selected FILE base".into(),
    };
    let file_commit = engine.change_file_working(
        &commit, &principal, storage_contract::OperationId::generate().unwrap(),
    ).unwrap();
    let newer_base = engine.file_draft_base(&fixture.targets[0]).unwrap();
    assert_ne!(newer_base.version_id(), initial_base.version_id());
    write_draft(&mut engine, &fixture.targets[0], NEW_FIRST_BASE);
    write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_ORIGINAL);
    assert_eq!(inventory(&fixture.snapshot_path()), original_directory);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.operation(file_commit.operation_id).unwrap(), file_commit);
    // Preserve the request order while the complete manifest remains canonical
    // by lookup path. These are two real same-base manual saves, not mocked results.
    let selected = [
        selection(&engine, &fixture.targets[1]),
        selection(&engine, &fixture.targets[0]),
    ];
    let pending = PendingDirectoryDrafts::capture(
        &engine, fixture.intent(&selected, &principal),
    ).unwrap();
    let context = engine.planning_context();
    let bound = BoundDirectoryRequest::capture(&pending, &context).unwrap();
    let before = Arc::clone(&engine.state);
    let media = fixture.media();
    let original = inventory(&fixture.snapshot_path());
    let maximum_entries = engine.config.max_entries;
    let bytes = codec::encode_request(
        bound.request(), DESCRIPTION_BYTES, maximum_entries,
    ).unwrap();
    let decoded = codec::decode_request(&bytes, DESCRIPTION_BYTES, maximum_entries).unwrap();
    assert_eq!(&decoded, bound.request());
    bound.require_request(&context, &decoded).unwrap();
    for (decoded, actual) in decoded.selected.iter().zip(&selected) {
        assert_eq!(decoded.target, actual.target);
        assert_eq!(decoded.expectation(), actual.expected);
    }
    assert!(matches!(
        codec::decode_request(&bytes, bytes.len() - 1, maximum_entries),
        Err(DescriptionError::Budget),
    ));
    assert!(matches!(
        codec::decode_request(&bytes, DESCRIPTION_BYTES, 1),
        Err(DescriptionError::Budget),
    ));
    let mut spaced = vec![b' '];
    spaced.extend_from_slice(&bytes);
    assert!(matches!(
        codec::decode_request(&spaced, DESCRIPTION_BYTES, maximum_entries),
        Err(DescriptionError::Canonical),
    ));
    let mut value: json_codec::Value = storage_contract::decode_strict(&bytes).unwrap();
    value["selected"][0]["working_after"] = json_codec::json!({"cleared": true});
    let extra = storage_contract::canonical_json(&value).unwrap();
    assert!(matches!(
        codec::decode_request(&extra, DESCRIPTION_BYTES, maximum_entries),
        Err(DescriptionError::Decode(_)),
    ));
    let mut empty: json_codec::Value = storage_contract::decode_strict(&bytes).unwrap();
    empty["selected"][0]["content"] = json_codec::json!({"kind": "unborn_empty"});
    let empty_bytes = storage_contract::canonical_json(&empty).unwrap();
    let empty_description = codec::decode_request(
        &empty_bytes, DESCRIPTION_BYTES, maximum_entries,
    ).unwrap();
    // Strict shape is not permission to relabel this genuinely Born selection.
    assert!(matches!(bound.require_request(&context, &empty_description),
        Err(ReconstructionError::Request)));
    empty["selected"][0]["content"]["version"] = json_codec::json!(newer_base.version_id());
    let extra_empty = storage_contract::canonical_json(&empty).unwrap();
    assert!(matches!(
        codec::decode_request(&extra_empty, DESCRIPTION_BYTES, maximum_entries),
        Err(DescriptionError::Decode(_)),
    ));

    // The second disk is genuinely part of this Genesis, but it cannot replace
    // this snapshot's owner merely by supplying another internally valid tuple.
    let other_id = engine.config.disks[1].disk_id.parse::<DiskId>().unwrap();
    let other = &engine.config.storage_roots[&other_id];
    let mut wrong_owner = decoded.clone();
    wrong_owner.source.host.registration = other.registration.clone();
    wrong_owner.source.host.root_entity = other.root_entity;
    wrong_owner.source.host.common_data = other.roots.common_data.clone();
    wrong_owner.source.root.root_id = other.root_id;
    wrong_owner.source.root.directory_id = other.roots.common_data.directory_id;
    wrong_owner.source.root.expected_root_generation = other.generation;
    wrong_owner.validate(maximum_entries).unwrap();
    assert!(matches!(bound.require_request(&context, &wrong_owner),
        Err(ReconstructionError::Request)));

    let mut wrong_generation = decoded.clone();
    wrong_generation.source.root.expected_root_generation =
        wrong_generation.source.root.expected_root_generation.next().unwrap();
    assert!(matches!(bound.require_request(&context, &wrong_generation),
        Err(ReconstructionError::Request)));
    let mut wrong_namespace = decoded.clone();
    wrong_namespace.source.namespace_generation =
        wrong_namespace.source.namespace_generation.next().unwrap();
    assert!(matches!(bound.require_request(&context, &wrong_namespace),
        Err(ReconstructionError::Request)));
    let mut wrong_revision = decoded.clone();
    wrong_revision.selected[0].revision = wrong_revision.selected[0].revision.next().unwrap();
    assert!(matches!(bound.require_request(&context, &wrong_revision),
        Err(ReconstructionError::Request)));
    let mut wrong_publication = decoded.clone();
    wrong_publication.source.handle.publication = file_commit.operation_id;
    assert!(matches!(bound.require_request(&context, &wrong_publication),
        Err(ReconstructionError::Request)));
    let mut wrong_principal = decoded.clone();
    wrong_principal.principal = "another-actor".parse().unwrap();
    assert!(matches!(bound.require_request(&context, &wrong_principal),
        Err(ReconstructionError::Request)));

    let mut swapped = decoded.clone();
    swapped.selected.swap(0, 1);
    let swapped_bytes = codec::encode_request(
        &swapped, DESCRIPTION_BYTES, maximum_entries,
    ).unwrap();
    assert_ne!(swapped_bytes, bytes);
    assert!(matches!(bound.require_request(&context, &swapped),
        Err(ReconstructionError::Request)));
    let mut alias = decoded.clone();
    alias.selected[1].target.file = alias.selected[0].target.file;
    assert!(matches!(alias.validate(maximum_entries), Err(DescriptionError::Selection)));
    let mut path_alias = decoded.clone();
    path_alias.selected[1].target.path = path_alias.selected[0].target.path.clone();
    assert!(matches!(path_alias.validate(maximum_entries), Err(DescriptionError::Selection)));
    let mut entry_alias = decoded.clone();
    entry_alias.selected[1].target.entry = entry_alias.selected[0].target.entry;
    assert!(matches!(entry_alias.validate(maximum_entries), Err(DescriptionError::Selection)));
    let mut source_alias = decoded.clone();
    source_alias.source.handle.node_id = SnapshotNodeId::generate().unwrap();
    assert!(matches!(source_alias.validate(maximum_entries), Err(DescriptionError::Source)));

    let construction = retained(&engine, &decoded);
    let projection = bound.reconstruct_proposed(&context, &construction).unwrap();
    assert_eq!(projection.manifest().operation_id(), pending.directory().operation());
    assert_ne!(projection.manifest().entries()[0], pending.original().manifest().entries()[0]);
    assert_eq!(projection.manifest().entries()[1], pending.original().manifest().entries()[1]);
    let SnapshotEntryContent::File { material, source } =
        projection.manifest().entries()[0].content else {
            panic!("the real selected FILE remains a file");
        };
    assert_eq!(material.version_id, newer_base.version_id());
    assert_eq!(source, crate::working_tree::snapshot::SnapshotFileSource::Container {
        branch: newer_base.branch_id(), node: newer_base.node_id(),
    });
    assert_eq!(projection.manifest().entries()[0].path, fixture.targets[0].path);
    assert_eq!(projection.manifest().entries()[1].path, fixture.targets[1].path);
    assert_eq!(projection.manifest().node_id(), construction.node);
    assert_ne!(projection.manifest().node_id(), fixture.source.node_id);
    assert_eq!(
        bound.reconstruct_proposed(&context, &construction).unwrap().manifest().bytes(),
        projection.manifest().bytes(),
    );

    let mut missing = construction.clone();
    missing.files.pop();
    assert!(matches!(bound.reconstruct_proposed(&context, &missing),
        Err(ReconstructionError::Description(DescriptionError::Cardinality))));
    let mut reversed = construction.clone();
    reversed.files.swap(0, 1);
    assert!(matches!(bound.reconstruct_proposed(&context, &reversed),
        Err(ReconstructionError::Description(DescriptionError::Outcome))));
    let mut replayed = construction.clone();
    replayed.files[1].outcome = replayed.files[0].outcome;
    assert!(matches!(bound.reconstruct_proposed(&context, &replayed),
        Err(ReconstructionError::Description(DescriptionError::Outcome))));

    let FileOutcome::Retained { base, material } = construction.files[0].outcome else {
        panic!("retained helper only observes real baseline material");
    };
    let mut reused_version = construction.clone();
    reused_version.files[0].outcome = FileOutcome::Published {
        base, branch: base.branch_id(),
        node: FileNodeId::Version(material.version_id), material,
    };
    assert!(matches!(bound.reconstruct_proposed(&context, &reused_version),
        Err(ReconstructionError::Description(DescriptionError::Outcome))));
    let mut wrong_branch = construction.clone();
    wrong_branch.files[0].outcome = FileOutcome::Published {
        base,
        branch: newer_base.branch_id(),
        node: FileNodeId::Version(newer_base.version_id()),
        material: SnapshotFile { version_id: newer_base.version_id(), ..material },
    };
    // The alternative branch/version are real, but belong to the other FILE.
    assert!(matches!(bound.reconstruct_proposed(&context, &wrong_branch),
        Err(ReconstructionError::Description(DescriptionError::Outcome))));
    let mut pretend_first = construction.clone();
    pretend_first.files[0].outcome = FileOutcome::FirstPublished {
        branch: base.branch_id(), node: FileNodeId::Version(material.version_id), material,
    };
    assert!(matches!(bound.reconstruct_proposed(&context, &pretend_first),
        Err(ReconstructionError::Description(DescriptionError::Outcome))));

    // A real old binding cannot be relabelled as the proposed node. This is a
    // rejection test, not a forged successful prepared/Published decision.
    let decision = DecisionDescription {
        format: DecisionFormat::DirectoryDraftDecision,
        construction,
        entries: projection.manifest().entries().to_vec(),
        material: projection.manifest().reference(),
        binding: pending.original().binding(),
    };
    let decision_bytes = storage_contract::canonical_json(&decision).unwrap();
    assert!(matches!(
        codec::decode_decision(&decision_bytes, DESCRIPTION_BYTES, maximum_entries),
        Err(DescriptionError::Source),
    ));
    assert!(matches!(bound.compare_decision_description(&context, &decision),
        Err(ReconstructionError::Description(DescriptionError::Source))));
    let mut decision_missing = decision.clone();
    decision_missing.construction.files.clear();
    let decision_missing = storage_contract::canonical_json(&decision_missing).unwrap();
    assert!(matches!(
        codec::decode_decision(&decision_missing, DESCRIPTION_BYTES, maximum_entries),
        Err(DescriptionError::Cardinality),
    ));
    for entry in projection.manifest().entries() {
        let SnapshotEntryContent::File { material, .. } = entry.content else {
            panic!("fixture contains two actual FILE originals");
        };
        assert!(engine.state().versions.contains_key(&material.version_id.to_string()));
    }
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert_eq!(fixture.media(), media);
    assert_eq!(inventory(&fixture.snapshot_path()), original);
}
