//! Frozen publication objects and a separate flat namespace oracle.
//!
//! Page boundaries are publication history, not a function of logical entries.
//! Restore the recorded objects; never rebuild a recorded commit's root.
use crate::journal::AuthenticatedMetadataObject;
use crate::namespace::{DraftSource, LoaderSource, NamespaceObjects, NamespaceView};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;
use storage_contract::{DiskConfig, Entry, NamespaceRefKind, NamespaceReference, State};

#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CanonicalPublicationFixture {
    pub(crate) commit_namespaces: BTreeMap<String, Vec<Entry>>,
    pub(crate) namespace_objects: BTreeMap<String, String>,
    pub(crate) state_bytes: String,
    pub(crate) snapshot_hash: String,
    pub(crate) snapshot_objects: BTreeMap<String, String>,
    pub(crate) history_roots: crate::metadata_map::Reference,
    pub(crate) history_objects: BTreeMap<String, String>,
}

impl CanonicalPublicationFixture {
    pub(crate) fn read() -> Self {
        json_codec::from_slice(include_bytes!(
            "journal/fixtures/canonical_publication.json"
        ))
        .unwrap()
    }

    /// Validate the self-contained recorded graph against the flat oracle,
    /// then publish the exact bytes into memory and optional member disks.
    pub(crate) fn restore(&self, members: &[DiskConfig]) -> State {
        let state: State = json_codec::from_str(&self.state_bytes).unwrap();
        let objects = decode_namespace_objects(&self.namespace_objects);
        // Do not fall back to the shared fixture store: another test must not
        // conceal an incomplete recorded closure.
        let missing = LoaderSource(|_: &NamespaceReference| {
            Err(crate::Error::new(
                storage_contract::ErrorCode::ObjectCorrupt,
                "namespace object is absent from the recorded fixture",
            ))
        });
        let source = DraftSource {
            fresh: &objects,
            base: &missing,
        };
        assert_eq!(self.commit_namespaces.len(), state.commits.len());
        for (commit_id, commit) in state.commits.iter() {
            let entries = &self.commit_namespaces[commit_id];
            crate::namespace::verify_namespace(&commit.namespace, &source, entries.len() as u64)
                .unwrap();
            assert_eq!(
                NamespaceView::new(&commit.namespace, &source)
                    .collect_entries()
                    .unwrap(),
                *entries,
                "{commit_id}"
            );
        }
        for object in objects.objects() {
            object.store(members).unwrap();
        }
        crate::namespace::fixture_store::register(objects);
        state
    }
}

fn decode_namespace_objects(recorded: &BTreeMap<String, String>) -> NamespaceObjects {
    let mut objects = NamespaceObjects::new();
    for (address, bytes) in recorded {
        let (kind, hash) = address.split_once('/').unwrap();
        let kind = match kind {
            storage_contract::NAMESPACE_PAGE_KIND => NamespaceRefKind::Page,
            storage_contract::NAMESPACE_INDEX_KIND => NamespaceRefKind::Index,
            other => panic!("unexpected fixture namespace object kind {other}"),
        };
        let object = AuthenticatedMetadataObject::authenticate(
            kind.as_str(),
            hash,
            json_codec::from_str(bytes).unwrap(),
        )
        .unwrap();
        assert_eq!(object.bytes(), bytes.as_bytes(), "{address}");
        objects
            .install(
                &NamespaceReference {
                    kind,
                    hash: hash.to_owned(),
                },
                object,
            )
            .unwrap();
    }
    objects
}

fn split_page_fixture(
    entries: &[Entry],
) -> (storage_contract::CommitNamespace, BTreeMap<String, String>) {
    use crate::journal::PreparedMetadataObject;
    use storage_contract::{
        CommitNamespace, NamespaceIndex, NamespaceIndexChild, NamespacePage, NamespacePageEntry,
    };

    // Two half-full pages are valid after local splits. A fresh greedy build
    // instead uses 128 + 2, so rebuilding entries loses the recorded identity.
    let mut recorded = BTreeMap::new();
    let mut children = Vec::new();
    for chunk in entries.chunks(65) {
        let page = NamespacePage {
            format: Default::default(),
            entries: chunk
                .iter()
                .map(|entry| NamespacePageEntry::File {
                    entry_id: entry.entry_id.clone(),
                    name: entry.location().file_component(),
                    file_id: entry.file_id.clone(),
                    version_id: entry.version_id.clone(),
                    size: entry.size,
                })
                .collect(),
        };
        let object = PreparedMetadataObject::new(
            storage_contract::NAMESPACE_PAGE_KIND,
            &page,
            crate::disk::MAX_METADATA,
        )
        .unwrap();
        children.push(NamespaceIndexChild {
            first_key: chunk[0].lookup_key().to_owned(),
            entries: chunk.len() as u64,
            files: chunk.len() as u64,
            reference: NamespaceReference {
                kind: NamespaceRefKind::Page,
                hash: object.hash.clone(),
            },
        });
        recorded.insert(
            format!("{}/{}", object.kind, object.hash),
            String::from_utf8(object.bytes).unwrap(),
        );
    }
    let object = PreparedMetadataObject::new(
        storage_contract::NAMESPACE_INDEX_KIND,
        &NamespaceIndex {
            format: Default::default(),
            children,
        },
        crate::disk::MAX_METADATA,
    )
    .unwrap();
    let namespace = CommitNamespace {
        root: NamespaceReference {
            kind: NamespaceRefKind::Index,
            hash: object.hash.clone(),
        },
        entries: entries.len() as u64,
    };
    recorded.insert(
        format!("{}/{}", object.kind, object.hash),
        String::from_utf8(object.bytes).unwrap(),
    );
    (namespace, recorded)
}

#[test]
fn recorded_namespace_objects_keep_non_bulk_page_boundaries_and_exact_disk_bytes() {
    use storage_contract::NamespaceEntryKind;

    let entries: Vec<Entry> = (0..130)
        .map(|index| Entry {
            entry_id: format!("entry_{index:04}"),
            path: name_policy::PortablePath::parse(&format!("file-{index:04}.txt")).unwrap(),
            kind: NamespaceEntryKind::File,
            file_id: format!("file_{index:04}"),
            version_id: format!("version_{index:04}"),
            size: index,
        })
        .collect();
    let (namespace, recorded) = split_page_fixture(&entries);
    let (bulk, _) = crate::namespace::fixture_namespace(&entries);
    assert_ne!(namespace.root, bulk.root);

    let objects = decode_namespace_objects(&recorded);
    let temp = tempfile::tempdir().unwrap();
    let config = storage_contract::Config::development([temp.path().join("member")]);
    for object in objects.objects() {
        object.store(&config.disks).unwrap();
        let actual = std::fs::read(crate::journal::object_path(
            &config.disks[0],
            object.kind,
            &object.hash,
        ))
        .unwrap();
        assert_eq!(
            actual,
            recorded[&format!("{}/{}", object.kind, object.hash)].as_bytes()
        );
    }
    let source = crate::history_package::fixture_member_source(&config.disks);
    assert_eq!(
        crate::namespace::verify_namespace(&namespace, &source, 130).unwrap(),
        130
    );
    assert_eq!(
        NamespaceView::new(&namespace, &source)
            .collect_entries()
            .unwrap(),
        entries
    );
}

#[test]
#[should_panic(expected = "namespace object is absent from the recorded fixture")]
fn publication_fixture_missing_objects_do_not_resolve_from_other_tests() {
    let mut fixture = CanonicalPublicationFixture::read();
    let state = fixture.restore(&[]);
    let root = &state.commits.values().next().unwrap().namespace.root;
    // The first restore registered this exact root in the shared fixture store.
    // Removing it must nevertheless make the next isolated admission fail.
    fixture
        .namespace_objects
        .remove(&format!("{}/{}", root.kind.as_str(), root.hash))
        .unwrap();
    fixture.restore(&[]);
}
