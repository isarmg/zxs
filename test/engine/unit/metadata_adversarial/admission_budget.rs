//! Regression for enforcing namespace depth before pre-admission projections.
use super::*;
use std::cell::Cell;
use storage_contract::{
    CommitNamespace, NAMESPACE_INDEX_DEPTH, NAMESPACE_INDEX_KIND, NAMESPACE_PAGE_KIND,
    NamespaceIndex, NamespaceIndexChild, NamespacePage, NamespacePageEntry, NamespaceRefKind,
    NamespaceReference, RepositoryName,
};

fn page(disk: &DiskConfig, entries: Vec<NamespacePageEntry>) -> NamespaceReference {
    NamespaceReference {
        kind: NamespaceRefKind::Page,
        hash: store_object(
            disk,
            NAMESPACE_PAGE_KIND,
            &NamespacePage {
                format: Default::default(),
                entries,
            },
        )
        .unwrap(),
    }
}

fn index(disk: &DiskConfig, children: Vec<NamespaceIndexChild>) -> NamespaceReference {
    NamespaceReference {
        kind: NamespaceRefKind::Index,
        hash: store_object(
            disk,
            NAMESPACE_INDEX_KIND,
            &NamespaceIndex {
                format: Default::default(),
                children,
            },
        )
        .unwrap(),
    }
}

fn child(name: &str, reference: NamespaceReference) -> NamespaceIndexChild {
    NamespaceIndexChild {
        first_key: name.into(),
        entries: 1,
        files: 0,
        reference,
    }
}

fn deep_child(disk: &DiskConfig, name: &str, identity: u64) -> NamespaceIndexChild {
    let empty = page(disk, Vec::new());
    let leaf = page(
        disk,
        vec![NamespacePageEntry::Directory {
            entry_id: format!("entry_{identity:032x}"),
            name: name.into(),
            entries: 0,
            files: 0,
            subtree: empty,
        }],
    );
    let mut current = child(name, leaf);
    for _ in 0..NAMESPACE_INDEX_DEPTH - 1 {
        current = child(name, index(disk, vec![current]));
    }
    current
}

fn has_depth_cause(error: &Error) -> bool {
    let mut source: &(dyn std::error::Error + 'static) = error;
    loop {
        if let Some(cause) = source.downcast_ref::<Error>()
            && cause.code() == ErrorCode::ObjectCorrupt
            && cause.message() == "namespace index exceeds the depth bound"
        {
            return true;
        }
        match source.source() {
            Some(next) => source = next,
            None => return false,
        }
    }
}

fn save_counterexample(disk: &DiskConfig, snapshot: &Snapshot) {
    let Some(directory) = std::env::var_os("ZXS_METADATA_ADVERSARIAL_CASE_DIR") else {
        return;
    };
    let mut objects = BTreeMap::new();
    for kind in [
        "namespace-page",
        "namespace-index",
        "state-page",
        "metadata-index",
        "event-segment",
        "operation-segment",
        "state-control-auth",
    ] {
        for hash in object_storage::list_object_hashes(disk, kind).unwrap() {
            let value: json_codec::Value = load_object(disk, kind, &hash).unwrap();
            objects.insert(format!("{kind}/{hash}"), value);
        }
    }
    let evidence = json_codec::json!({
        "baseline":"13b664e9fd41a356ffb7ed982d21e97abb123626",
        "entry":"journal::resolve_snapshot", "snapshot":snapshot, "objects":objects,
        "expected":"ObjectCorrupt depth rejection before requesting namespace object 5",
        "test_side_read_allowance":NAMESPACE_INDEX_DEPTH
    });
    std::fs::write(
        std::path::Path::new(&directory).join("admission-depth-input.json"),
        canonical_json(&evidence).unwrap(),
    )
    .unwrap();
}

#[test]
fn snapshot_admission_applies_namespace_depth_before_projection_reads() {
    if let Some(path) = std::env::var_os("ZXS_METADATA_ADVERSARIAL_REPLAY_INPUT") {
        let input: json_codec::Value = decode_strict(&std::fs::read(path).unwrap()).unwrap();
        let snapshot: Snapshot = json_codec::from_value(input["snapshot"].clone()).unwrap();
        let objects = input["objects"].as_object().unwrap();
        assert_depth_before_projection(snapshot, |kind, hash| {
            let address = format!("{kind}/{hash}");
            let value = objects
                .get(&address)
                .expect("counterexample reference closure");
            AuthenticatedMetadataObject::authenticate(kind, hash, value.clone())
        });
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("member")]);
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("depth-budget").unwrap(),
            "P01",
            1,
            "admin",
            "op_00000000000000000000000000000001".parse().unwrap(),
        )
        .unwrap();
    let mut state = engine.state().clone();
    drop(engine);
    let disk = &config.disks[0];
    let root = index(
        disk,
        vec![deep_child(disk, "a", 1), deep_child(disk, "b", 2)],
    );
    // Both branches have equal height, all keys/counts/IDs are consistent,
    // and every object has its actual domain hash. Only index depth is invalid.
    assert_eq!(state.commits.len(), 1);
    let commit = state.commits.keys().next().unwrap().clone();
    state.commits.get_mut(&commit).unwrap().namespace = CommitNamespace { root, entries: 2 };
    let snapshot_hash = store_snapshot(&config.disks, &state).unwrap();
    let snapshot: Snapshot = load_object(disk, "snapshot", &snapshot_hash).unwrap();
    save_counterexample(disk, &snapshot);
    assert_depth_before_projection(snapshot, |kind, hash| {
        load_authenticated_object(disk, kind, hash)
    });
}

fn assert_depth_before_projection(
    snapshot: Snapshot,
    mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) {
    let attempted = Cell::new(0usize);
    let loaded = Cell::new(0usize);
    let error = resolve_snapshot(snapshot, |kind, hash| {
        let namespace = matches!(kind, NAMESPACE_PAGE_KIND | NAMESPACE_INDEX_KIND);
        if namespace {
            attempted.set(attempted.get() + 1);
            // This is a test-side I/O allowance, not a production hook. The
            // invalid fifth level needs no read to establish its depth error.
            if attempted.get() > NAMESPACE_INDEX_DEPTH {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "adversarial namespace read cap",
                ));
            }
        }
        let object = load(kind, hash)?;
        if namespace {
            loaded.set(loaded.get() + 1);
        }
        Ok(object)
    })
    .unwrap_err();
    eprintln!(
        "admission-depth attempted={} loaded={} bound={} error={error:?}",
        attempted.get(),
        loaded.get(),
        NAMESPACE_INDEX_DEPTH,
    );
    assert!(
        has_depth_cause(&error),
        "depth must fail before the I/O allowance: {error:?}"
    );
    assert!(attempted.get() <= NAMESPACE_INDEX_DEPTH);
    assert_eq!(loaded.get(), NAMESPACE_INDEX_DEPTH);
}
