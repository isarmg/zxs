use super::*;
use crate::{
    Config,
    journal::{load_object, object_path},
};
use crate::{canonical_json, disk::MAX_METADATA, encode_hashed_object, object_hash};
use json_codec::json;
use std::collections::{BTreeMap, BTreeSet};

// Independent current-format expectations, not implementation visibility.
const PAGE_ENTRIES: usize = 128;
const PAGE_TARGET_BYTES: usize = 64 * 1024;
const RADIX_DEPTH: usize = 64;

fn key_route(key: &str) -> String {
    blake3::hash(key.as_bytes()).to_hex().to_string()
}

type ResolvedGraph = (HistoryManifest, Vec<(String, String, Value)>);

fn resolve_with_objects(
    root: Value,
    mut load: impl FnMut(&str, &str) -> Result<Value>,
) -> Result<ResolvedGraph> {
    let hash = object_hash("history-manifest", &root)?;
    let root = AuthenticatedMetadataObject::authenticate("history-manifest", &hash, root)?;
    let (manifest, objects) = super::resolve_with_objects(root, |kind, hash| {
        AuthenticatedMetadataObject::authenticate(kind, hash, load(kind, hash)?)
    })?;
    Ok((
        manifest,
        objects
            .into_iter()
            .map(|object| {
                (
                    object.kind().to_owned(),
                    object.hash().to_owned(),
                    object.value().clone(),
                )
            })
            .collect(),
    ))
}

fn resolve(root: Value, load: impl FnMut(&str, &str) -> Result<Value>) -> Result<HistoryManifest> {
    resolve_with_objects(root, load).map(|(manifest, _)| manifest)
}

fn manifest() -> HistoryManifest {
    let mut value = json!({
        "repository": {
            "repository_id": "repository",
            "name": "history graph",
            "history_tree_id": "history",
            "main_branch_id": "branch",
            "primary_slot_id": "P01",
            "target_copies": 3,
            "smb_enabled": true,
            "smb_binding_generation": "1",
            "name_policy": "portable",
            "unicode_version": "15.1"
        },
        "history": {
            "history_tree_id": "history",
            "repository_id": "repository",
            "root_commit_id": "commit",
            "main_branch_id": "branch",
            "generation": "18446744073709551615",
            "state": "active",
            "provenance": null
        }
    });
    for field in Field::ALL.iter().map(|field| field.name()) {
        value[field] = json!({});
    }
    json_codec::from_value(value).unwrap()
}

fn encoded(
    manifest: &HistoryManifest,
    maximum: usize,
) -> (String, Value, BTreeMap<(String, String), Value>) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let disk = &config.disks[0];
    let (hash, prepared) = prepare(manifest, maximum).unwrap();
    for object in prepared {
        object.store(std::slice::from_ref(disk)).unwrap();
    }
    let root: Value = load_object(disk, "history-manifest", &hash).unwrap();
    let (_, objects) =
        resolve_with_objects(root.clone(), |kind, digest| load_object(disk, kind, digest)).unwrap();
    for (kind, digest, value) in &objects {
        assert_eq!(
            std::fs::read(object_path(disk, kind, digest)).unwrap(),
            canonical_json(value).unwrap()
        );
    }
    (
        hash,
        root,
        objects
            .into_iter()
            .map(|(kind, hash, value)| ((kind, hash), value))
            .collect(),
    )
}

fn object(values: &mut BTreeMap<(String, String), Value>, kind: &str, value: Value) -> Value {
    let hash = object_hash(kind, &value).unwrap();
    values.insert((kind.to_owned(), hash.clone()), value);
    json!({"kind": kind, "hash": hash})
}

fn read(root: Value, values: &BTreeMap<(String, String), Value>) -> Result<ResolvedGraph> {
    resolve_with_objects(root, |kind, hash| {
        values
            .get(&(kind.to_owned(), hash.to_owned()))
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::ObjectCorrupt,
                    "missing test graph object",
                )
            })
    })
}

#[test]
fn deterministic_bounded_graph_preserves_logical_wire_rules() {
    let mut manifest = manifest();
    for n in (0..1800).rev() {
        manifest
            .file_notes
            .insert(format!("file-{n:05}"), "文".repeat(90));
    }
    manifest
        .file_selection_generations
        .insert("large".to_owned(), u64::MAX);
    manifest
        .directory_selection_generations
        .insert("unsafe-number".to_owned(), (1 << 53) + 1);
    assert!(canonical_json(&manifest).unwrap().len() > PAGE_TARGET_BYTES);
    let (hash, root, values) = encoded(&manifest, PAGE_TARGET_BYTES);
    let (large_limit_hash, prepared) = prepare(&manifest, MAX_METADATA).unwrap();
    assert_eq!(hash, large_limit_hash);
    assert_eq!(values.len(), prepared.len());
    let (resolved, reachable) = read(root.clone(), &values).unwrap();
    assert_eq!(
        canonical_json(&manifest).unwrap(),
        canonical_json(&resolved).unwrap()
    );
    assert_eq!(
        reachable.last().unwrap(),
        &("history-manifest".to_owned(), hash, root.clone())
    );
    assert_eq!(
        root.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["format", "history", "maps", "repository"])
    );
    assert_eq!(root["maps"].as_object().unwrap().len(), Field::ALL.len());
    assert_eq!(root["history"]["generation"], u64::MAX.to_string());
    let mut seen = BTreeSet::new();
    for (kind, hash, value) in reachable {
        assert!(seen.insert((kind.clone(), hash)));
        assert!(canonical_json(&value).unwrap().len() <= PAGE_TARGET_BYTES);
        if kind == "state-page" {
            assert!(value.as_object().unwrap().len() <= PAGE_ENTRIES);
        } else if kind == "metadata-index" {
            assert_eq!(
                value
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["children", "prefix"])
            );
            let children = value["children"].as_object().unwrap();
            assert!(children.len() <= 16);
            for reference in children.values() {
                assert!(seen.contains(&(
                    reference["kind"].as_str().unwrap().to_owned(),
                    reference["hash"].as_str().unwrap().to_owned(),
                )));
            }
        }
    }
}

#[test]
fn singleton_over_page_target_retains_existing_entity_ceiling() {
    let mut manifest = manifest();
    manifest.file_notes.insert(
        "large-single-entity".to_owned(),
        "x".repeat(PAGE_TARGET_BYTES),
    );
    let (hash, root, values) = encoded(&manifest, MAX_METADATA);
    assert_eq!(root["maps"]["file_notes"]["kind"], "state-page");
    assert!(read(root, &values).is_ok());
    let error = match prepare(&manifest, PAGE_TARGET_BYTES) {
        Err(error) => error,
        Ok(_) => panic!("a caller's smaller reader limit must still apply"),
    };
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert_eq!(hash, prepare(&manifest, MAX_METADATA / 2).unwrap().0);
}

#[test]
fn truly_oversized_singleton_fails_before_any_object_can_be_installed() {
    let mut manifest = manifest();
    manifest
        .file_notes
        .insert("oversized".to_owned(), "x".repeat(MAX_METADATA + 1));
    let error = match prepare(&manifest, usize::MAX) {
        Err(error) => error,
        Ok(_) => panic!("a caller cannot enlarge the immutable object reader limit"),
    };
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert!(error.message().contains("one metadata entity"));
}

/// This is a codec capacity test, not a semantic notes-policy or an
/// end-to-end incremental-performance benchmark. Run it alone: the
/// logical projection and the frozen/resolved graph remain O(total) RAM.
#[test]
#[ignore = "real >64 MiB codec scale test; allocates a large logical manifest and graph"]
fn distributed_history_larger_than_real_reader_limit_roundtrips() {
    let mut manifest = manifest();
    const ENTITIES: usize = 1300;
    const VALUE_BYTES: usize = 53 * 1024;
    for n in 0..ENTITIES {
        manifest
            .file_notes
            .insert(format!("capacity-{n:05}"), "x".repeat(VALUE_BYTES));
    }
    let (flat, logical_hash) = encode_hashed_object("history-manifest", &manifest).unwrap();
    let flat_bytes = flat.len();
    assert!(flat_bytes > MAX_METADATA);
    drop(flat);
    let flat_error = match PreparedMetadataObject::new("history-manifest", &manifest, MAX_METADATA)
    {
        Err(error) => error,
        Ok(_) => panic!("a single object must reject the oversized expanded manifest"),
    };
    assert_eq!(
        flat_error.code(),
        storage_contract::ErrorCode::ResourceLimit
    );

    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let disk = &config.disks[0];
    let (root_hash, prepared) = prepare(&manifest, MAX_METADATA).unwrap();
    let prepared_hashes = prepared
        .iter()
        .map(|object| object.hash.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(prepared_hashes.len(), prepared.len());
    // Store one disposable replica, releasing prepared bytes as we go.
    // No database, source tree, real pool, or version bodies are copied.
    for object in prepared {
        object.store(std::slice::from_ref(disk)).unwrap();
    }
    let (tighter_root, tighter_objects) = prepare(&manifest, PAGE_TARGET_BYTES).unwrap();
    assert_eq!(tighter_root, root_hash);
    assert_eq!(
        tighter_objects
            .iter()
            .map(|object| object.hash.clone())
            .collect::<BTreeSet<_>>(),
        prepared_hashes
    );
    drop(tighter_objects);
    let root = load_object(disk, "history-manifest", &root_hash).unwrap();
    let (resolved, objects) =
        resolve_with_objects(root, |kind, hash| load_object(disk, kind, hash)).unwrap();
    assert_eq!(resolved.file_notes, manifest.file_notes);
    assert_eq!(resolved.file_notes.len(), ENTITIES);
    // Compare complete logical canonical identity without retaining two
    // additional >64 MiB byte arrays at once.
    drop(manifest);
    let (resolved_bytes, resolved_hash) =
        encode_hashed_object("history-manifest", &resolved).unwrap();
    assert_eq!(resolved_hash, logical_hash);
    assert_eq!(resolved_bytes.len(), flat_bytes);
    drop(resolved_bytes);
    assert_eq!(objects.len(), prepared_hashes.len());
    let mut total_graph_bytes = 0u64;
    let mut largest_object_bytes = 0u64;
    let mut leaf_pages = 0usize;
    for (kind, hash, _) in &objects {
        assert!(prepared_hashes.contains(hash));
        let bytes = std::fs::metadata(object_path(disk, kind, hash))
            .unwrap()
            .len();
        assert!(bytes <= MAX_METADATA as u64);
        if kind == "state-page" {
            leaf_pages += 1;
            assert!(bytes <= PAGE_TARGET_BYTES as u64);
        }
        total_graph_bytes += bytes;
        largest_object_bytes = largest_object_bytes.max(bytes);
    }
    println!(
        "history codec capacity: flat_bytes={flat_bytes}, reader_limit={MAX_METADATA}, \
             graph_objects={}, leaf_pages={leaf_pages}, total_graph_bytes={total_graph_bytes}, \
             largest_object_bytes={largest_object_bytes}",
        objects.len()
    );
}

#[test]
fn expanded_logical_model_is_not_a_paged_root() {
    let root = json_codec::to_value(manifest()).unwrap();
    let error = resolve_with_objects(root, |_, _| {
        panic!("invalid root shape must not load any dependencies")
    })
    .err()
    .unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::InvalidJson);
}

#[test]
fn root_requires_the_fixed_complete_schema_and_typed_references() {
    let (_, root, values) = encoded(&manifest(), MAX_METADATA);
    let mut missing = root.clone();
    missing["maps"]
        .as_object_mut()
        .unwrap()
        .remove("file_notes");
    assert!(read(missing, &values).is_err());
    let mut extra = root.clone();
    extra["maps"]["invented"] = root["maps"]["file_notes"].clone();
    assert!(read(extra, &values).is_err());
    let mut extra = root.clone();
    extra["unrecognized"] = json!(true);
    assert!(read(extra, &values).is_err());
    let mut missing_root_field = root.clone();
    missing_root_field
        .as_object_mut()
        .unwrap()
        .remove("history");
    assert!(read(missing_root_field, &values).is_err());
    let mut unknown_kind = root.clone();
    unknown_kind["maps"]["file_notes"]["kind"] = json!("snapshot");
    assert!(read(unknown_kind, &values).is_err());
    let mut extra_reference_field = root.clone();
    extra_reference_field["maps"]["file_notes"]["prefix"] = json!("");
    assert!(read(extra_reference_field, &values).is_err());
    for digest in ["../outside", &"A".repeat(64), &"0".repeat(63)] {
        let mut unsafe_address = root.clone();
        unsafe_address["maps"]["file_notes"]["hash"] = json!(digest);
        assert!(
            resolve(unsafe_address, |_, _| panic!(
                "unsafe address reached loader"
            ))
            .is_err()
        );
    }
}

#[test]
fn loader_cannot_substitute_another_valid_object() {
    let (_, root, _) = encoded(&manifest(), MAX_METADATA);
    let error = resolve(root, |_, _| Ok(json!({"unexpected": "entity"}))).unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ObjectCorrupt);
}

#[test]
fn corrupt_index_routes_prefixes_kinds_and_fields_fail_closed() {
    let (_, root, values) = encoded(&manifest(), MAX_METADATA);
    let key = "one-entity";
    let route = key_route(key);
    let digit = &route[..1];
    let wrong_digit = if digit == "0" { "1" } else { "0" };
    for invalid_index in 0..8 {
        let mut values = values.clone();
        let page = object(&mut values, "state-page", json!({key: "note"}));
        let mut children = Map::new();
        children.insert(digit.to_owned(), page.clone());
        let mut index = json!({"prefix": "", "children": children});
        match invalid_index {
            0 => index["prefix"] = json!("0"),
            1 => {
                index.as_object_mut().unwrap().remove("prefix");
            }
            2 => index["unknown_field"] = json!(true),
            3 => index["children"][wrong_digit] = page,
            4 => index["children"]["invalid-digit"] = page,
            5 => index["children"] = json!({}),
            6 => index["children"][digit]["kind"] = json!("history-manifest"),
            7 => index["children"][digit]["hash"] = json!("../outside"),
            _ => unreachable!(),
        }
        let mut altered_root = root.clone();
        altered_root["maps"]["file_notes"] = object(&mut values, "metadata-index", index);
        assert!(read(altered_root, &values).is_err(), "case {invalid_index}");
    }
}

#[test]
fn leaf_bounds_and_empty_nonroot_pages_are_validated_on_read() {
    let (_, root, values) = encoded(&manifest(), MAX_METADATA);
    let too_many = (0..=PAGE_ENTRIES)
        .map(|n| (format!("key-{n}"), json!("value")))
        .collect::<Map<_, _>>();
    let too_large = json!({
        "one": "x".repeat(PAGE_TARGET_BYTES / 2),
        "two": "x".repeat(PAGE_TARGET_BYTES / 2)
    });
    for page in [Value::Object(too_many), too_large, json!(["not a map"])] {
        let mut values = values.clone();
        let mut altered_root = root.clone();
        altered_root["maps"]["file_notes"] = object(&mut values, "state-page", page);
        assert!(read(altered_root, &values).is_err());
    }
    let mut values = values;
    let empty = object(&mut values, "state-page", json!({}));
    let mut altered_root = root;
    altered_root["maps"]["file_notes"] = object(
        &mut values,
        "metadata-index",
        json!({"prefix": "", "children": {"0": empty}}),
    );
    assert!(read(altered_root, &values).is_err());
}

#[test]
fn index_cannot_extend_beyond_the_full_key_digest() {
    let (_, mut root, mut values) = encoded(&manifest(), MAX_METADATA);
    let key = "deep-entity";
    let route = key_route(key);
    let mut reference = object(&mut values, "state-page", json!({key: "note"}));
    for depth in (0..=RADIX_DEPTH).rev() {
        let digit = if depth == RADIX_DEPTH {
            "0"
        } else {
            &route[depth..depth + 1]
        };
        let mut children = Map::new();
        children.insert(digit.to_owned(), reference);
        reference = object(
            &mut values,
            "metadata-index",
            json!({"prefix": &route[..depth], "children": children}),
        );
    }
    root["maps"]["file_notes"] = reference;
    assert!(read(root, &values).is_err());
}

#[test]
fn cached_objects_are_revalidated_at_each_logical_route() {
    let (_, root, values) = encoded(&manifest(), MAX_METADATA);
    let key = (0..)
        .map(|n| format!("shared-{n}"))
        .find(|key| key_route(key).starts_with("00"))
        .unwrap();
    for reuse_index in [false, true] {
        let mut values = values.clone();
        let page = object(&mut values, "state-page", json!({key.clone(): "note"}));
        let child = if reuse_index {
            object(
                &mut values,
                "metadata-index",
                json!({"prefix": "0", "children": {"0": page}}),
            )
        } else {
            page
        };
        let index = object(
            &mut values,
            "metadata-index",
            json!({
                "prefix": "",
                "children": {"0": child.clone(), "1": child}
            }),
        );
        let mut altered_root = root.clone();
        altered_root["maps"]["file_notes"] = index;
        let error = read(altered_root, &values).unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::HistoryPackageInvalid
        );
    }
}

#[test]
fn identical_field_maps_share_objects_and_empty_maps_have_no_index() {
    let manifest = manifest();
    let (_, root, values) = encoded(&manifest, MAX_METADATA);
    assert_eq!(values.len(), 2, "one shared empty leaf and one root");
    assert!(
        root["maps"]
            .as_object()
            .unwrap()
            .values()
            .all(|reference| reference["kind"] == "state-page")
    );
    assert_eq!(read(root, &values).unwrap().1.len(), 2);

    let mut manifest = manifest;
    for n in 0..300 {
        let key = format!("file-{n}");
        manifest.file_notes.insert(key.clone(), format!("note-{n}"));
        manifest.version_notes.insert(key, format!("note-{n}"));
    }
    let (_, root, values) = encoded(&manifest, MAX_METADATA);
    assert_eq!(root["maps"]["file_notes"], root["maps"]["version_notes"]);
    let (resolved, _) = read(root, &values).unwrap();
    assert_eq!(resolved.file_notes, manifest.file_notes);
    assert_eq!(resolved.version_notes, manifest.version_notes);
}

#[test]
fn changing_one_entity_reuses_unrelated_radix_pages() {
    let mut manifest = manifest();
    for n in 0..600 {
        manifest
            .file_notes
            .insert(format!("entity-{n}"), format!("note-{n}"));
    }
    let (old_hash, old_root, old_objects) = encoded(&manifest, MAX_METADATA);
    manifest
        .file_notes
        .insert("entity-10".to_owned(), "updated".to_owned());
    let (new_hash, new_root, new_objects) = encoded(&manifest, MAX_METADATA);
    assert_ne!(old_hash, new_hash);
    assert_eq!(old_root["maps"]["branches"], new_root["maps"]["branches"]);
    let old_pages = old_objects
        .keys()
        .filter(|(kind, _)| kind == "state-page")
        .collect::<BTreeSet<_>>();
    let new_pages = new_objects
        .keys()
        .filter(|(kind, _)| kind == "state-page")
        .collect::<BTreeSet<_>>();
    assert_eq!(old_pages.difference(&new_pages).count(), 1);
    assert_eq!(new_pages.difference(&old_pages).count(), 1);
}

// Fixed codec fixture, independent of random IDs and wall-clock time. It need
// not be a semantically valid history: this pins the logical/graph wire format.
#[test]
fn shared_manifest_projection_preserves_canonical_golden() {
    let mut logical = json_codec::to_value(manifest()).unwrap();
    logical["commits"] = json!({"commit": {
        "commit_id": "commit", "history_tree_id": "history", "kind": "save",
        "parent_commit_ids": [], "tree_id": "tree", "sequence": "18446744073709551615",
        "operation_id": "op_00000000000000000000000000000000", "author": "作者", "message": "quotes: \" / 📁",
        "entries": [], "pending_file_versions": {"file": "version"},
        "namespace_deletions": {"timestamp": "9007199254740993", "paths": []},
        "provenance": null
    }});
    logical["directory_trees"] = json!({"tree": {
        "tree_id": "tree", "directory_branch_id": "directory-branch", "parent_tree_id": null,
        "commit_id": "commit", "path": "目录/📁", "entries": [], "file_branches": {"file": "branch"},
        "inherited_files": {"file": "version"}, "sequence": "9007199254740993"
    }});
    logical["file_selection_generations"] = json!({"file": "18446744073709551615"});
    let manifest: HistoryManifest = json_codec::from_value(logical).unwrap();
    let bytes = canonical_json(&manifest).unwrap();
    let (root, _, objects) = encoded(&manifest, MAX_METADATA);
    let bundle = json!({
        "logical": String::from_utf8(bytes).unwrap(),
        "root": root,
        "objects": objects.into_iter().map(|((kind, hash), value)| json!({
            "kind": kind, "hash": hash,
            "bytes": String::from_utf8(canonical_json(&value).unwrap()).unwrap()
        })).collect::<Vec<_>>()
    });
    assert_eq!(
        canonical_json(&bundle).unwrap(),
        include_bytes!("manifest-sharing-golden.json")
    );
}

#[test]
fn manifest_shared_maps_keep_required_and_sparse_wire_rules() {
    let logical = json_codec::to_value(manifest()).unwrap();
    assert_eq!(logical["commits"], json!({}));
    assert!(logical.get("directory_trees").is_none());
    let mut explicit = logical.clone();
    explicit["directory_trees"] = json!({});
    let decoded: HistoryManifest = json_codec::from_value(explicit).unwrap();
    assert_eq!(json_codec::to_value(decoded).unwrap(), logical);
    let mut missing = logical;
    missing.as_object_mut().unwrap().remove("commits");
    assert!(json_codec::from_value::<HistoryManifest>(missing).is_err());
}

#[test]
fn content_state_manifest_graph_preserves_string_bytes_and_rejects_unknown_values() {
    let mut raw = json_codec::to_value(manifest()).unwrap();
    raw["content_states"] = json!({"a":"AVAILABLE","m":"MIGRATED","p":"PURGED"});
    let typed: HistoryManifest = json_codec::from_value(raw.clone()).unwrap();
    assert_eq!(
        canonical_json(&typed).unwrap(),
        canonical_json(&raw).unwrap()
    );
    assert_eq!(
        object_hash("history-manifest", &typed).unwrap(),
        object_hash("history-manifest", &raw).unwrap()
    );
    let (_, root, objects) = encoded(&typed, PAGE_TARGET_BYTES);
    let (resolved, _) = read(root.clone(), &objects).unwrap();
    assert_eq!(
        canonical_json(&resolved).unwrap(),
        canonical_json(&raw).unwrap()
    );
    for invalid in [
        json!("available"),
        json!("UNKNOWN"),
        json!({"AVAILABLE":null}),
        json!(null),
    ] {
        raw["content_states"] = json!({"a":invalid});
        assert!(json_codec::from_value::<HistoryManifest>(raw.clone()).is_err());
        // Keep every object hash self-consistent: rejection must come from the
        // resolved disposition schema, not merely from a damaged digest.
        let mut damaged_objects = objects.clone();
        let mut damaged_root = root.clone();
        damaged_root["maps"]["content_states"] = object(
            &mut damaged_objects,
            "state-page",
            raw["content_states"].clone(),
        );
        assert!(read(damaged_root, &damaged_objects).is_err());
    }
}

#[test]
fn markerless_and_previous_domain_history_roots_are_rejected_without_rewriting() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("disk")]);
    let disk = &config.disks[0];
    let (_, current, _) = encoded(&manifest(), MAX_METADATA);
    for mutation in ["missing_marker", "unknown_marker", "previous_index_domain"] {
        let mut old = current.clone();
        match mutation {
            "missing_marker" => {
                old.as_object_mut().unwrap().remove("format");
            }
            "unknown_marker" => old["format"] = json!("unknown"),
            _ => old["maps"]["file_notes"]["kind"] = json!("history-index"),
        }
        let hash = crate::journal::store_object(disk, "history-manifest", &old).unwrap();
        let path = object_path(disk, "history-manifest", &hash);
        let bytes = std::fs::read(&path).unwrap();
        let object =
            crate::journal::load_authenticated_object(disk, "history-manifest", &hash).unwrap();
        assert!(
            super::resolve_with_objects(object, |_, _| panic!(
                "old root must reject before dependency I/O"
            ))
            .is_err(),
            "{mutation}"
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[path = "descriptor_tests.rs"]
mod descriptor_tests;

#[test]
fn manifest_structural_equality_matches_canonical_closed_fields() {
    let bundle: Value =
        json_codec::from_slice(include_bytes!("manifest-sharing-golden.json")).unwrap();
    let logical: Value = json_codec::from_str(bundle["logical"].as_str().unwrap()).unwrap();
    let original: HistoryManifest = json_codec::from_value(logical.clone()).unwrap();
    assert_eq!(original, original.clone());
    let mut multiple = original.clone();
    let mut second = multiple.commits.get("commit").unwrap().clone();
    second.commit_id = "second".into();
    multiple.commits.insert("second".into(), second);
    let mut independently_allocated = multiple.clone();
    independently_allocated.commits = multiple
        .commits
        .iter()
        .rev()
        .map(|(id, record)| (id.clone(), record.clone()))
        .collect();
    assert!(
        !multiple
            .commits
            .shares_storage_with(&independently_allocated.commits)
    );
    assert_eq!(multiple, independently_allocated);
    assert_eq!(
        canonical_json(&multiple).unwrap(),
        canonical_json(&independently_allocated).unwrap()
    );
    // Exact large-u64 strings, Unicode, sparse fields, shared commits and trees.
    for (pointer, replacement) in [
        ("/repository/name", json!("changed")),
        ("/repository/smb_enabled", json!(false)),
        ("/history/generation", json!("18446744073709551614")),
        ("/history/main_branch_id", Value::Null),
        ("/commits/commit/message", json!("different 📁")),
        ("/commits/commit/sequence", json!("9007199254740993")),
        ("/directory_trees/tree/path", json!("other/目录")),
        (
            "/file_selection_generations/file",
            json!("9007199254740993"),
        ),
    ] {
        let mut changed = logical.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        let changed: HistoryManifest = json_codec::from_value(changed).unwrap();
        assert_ne!(original, changed, "{pointer}");
        assert_eq!(
            original == changed,
            canonical_json(&original).unwrap() == canonical_json(&changed).unwrap()
        );
    }
    let mut sparse = json_codec::to_value(manifest()).unwrap();
    sparse["directory_trees"] = json!({});
    let explicit: HistoryManifest = json_codec::from_value(sparse).unwrap();
    assert_eq!(manifest(), explicit);
    assert_eq!(
        canonical_json(&manifest()).unwrap(),
        canonical_json(&explicit).unwrap()
    );
}
