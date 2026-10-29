//! Authenticated object graphs entering the production namespace admission path.
use super::*;
use crate::journal::AuthenticatedMetadataObject;
use std::cell::Cell;
use storage_contract::{decode_strict, object_hash};

#[derive(Default)]
struct Graph {
    bytes: BTreeMap<(&'static str, String), Vec<u8>>,
    reads: Cell<usize>,
    read_bytes: Cell<usize>,
}

impl Graph {
    fn add(&mut self, kind: NamespaceRefKind, value: json_codec::Value) -> NamespaceReference {
        let hash = object_hash(kind.as_str(), &value).unwrap();
        self.bytes.insert(
            (kind.as_str(), hash.clone()),
            canonical_json(&value).unwrap(),
        );
        NamespaceReference { kind, hash }
    }

    fn page(&mut self, entries: Vec<json_codec::Value>) -> NamespaceReference {
        self.add(
            NamespaceRefKind::Page,
            json_codec::json!({"format":"namespace-page", "entries":entries}),
        )
    }

    fn index(&mut self, children: Vec<NamespaceIndexChild>) -> NamespaceReference {
        self.add(
            NamespaceRefKind::Index,
            json_codec::json!({"format":"namespace-index", "children":children}),
        )
    }

    fn source(&self) -> impl NamespaceSource + '_ {
        ObjectLoaderSource::new(|kind: &'static str, hash: &str| {
            self.reads.set(self.reads.get() + 1);
            let bytes = self
                .bytes
                .get(&(kind, hash.to_owned()))
                .expect("fixture object");
            self.read_bytes.set(self.read_bytes.get() + bytes.len());
            AuthenticatedMetadataObject::authenticate(kind, hash, decode_strict(bytes)?)
        })
    }

    fn verify(&self, reference: NamespaceReference, entries: u64, budget: u64) -> Result<u64> {
        verify_namespace(
            &CommitNamespace {
                root: reference,
                entries,
            },
            &self.source(),
            budget,
        )
    }
}

fn file(name: &str) -> json_codec::Value {
    json_codec::json!({
        "entry_id":format!("entry_{name}"), "name":name,
        "lookup_key":name_policy::lookup_key(name), "kind":"file",
        "file_id":format!("file_{name}"), "version_id":format!("ver_{name}"), "size":"0"
    })
}

fn child(key: &str, reference: NamespaceReference, entries: u64) -> NamespaceIndexChild {
    NamespaceIndexChild {
        first_key: key.into(),
        entries,
        files: entries,
        reference,
    }
}

fn two_pages(graph: &mut Graph) -> Vec<NamespaceIndexChild> {
    let left = graph.page(vec![file("a")]);
    let right = graph.page(vec![file("b")]);
    vec![child("a", left, 1), child("b", right, 1)]
}

fn rejects(result: Result<u64>, code: ErrorCode, message: &str) {
    let error = result.expect_err("invalid object graph must not enter trusted state");
    assert_eq!(error.code(), code, "{error:?}");
    assert_eq!(error.message(), message, "{error:?}");
}

#[test]
fn valid_graph_and_empty_root_have_independently_counted_totals() {
    let mut graph = Graph::default();
    let children = two_pages(&mut graph);
    let root = graph.index(children);
    assert_eq!(graph.verify(root, 2, 2).unwrap(), 2);
    assert_eq!(graph.reads.get(), 3);
    let empty = graph.page(vec![]);
    assert_eq!(graph.verify(empty, 0, 0).unwrap(), 0);
}

#[test]
fn authenticated_page_rejects_order_duplicates_and_entry_overflow() {
    for (entries, message) in [
        (
            vec![file("b"), file("a")],
            "namespace page keys are not strictly increasing",
        ),
        (
            vec![file("a"), file("A")],
            "namespace page keys are not strictly increasing",
        ),
        (
            (0..129).map(|n| file(&format!("k{n:03}"))).collect(),
            "namespace page exceeds the entry bound",
        ),
    ] {
        let mut graph = Graph::default();
        let count = entries.len() as u64;
        let root = graph.page(entries);
        rejects(
            graph.verify(root, count, count),
            ErrorCode::ObjectCorrupt,
            message,
        );
        assert_eq!(graph.reads.get(), 1);
    }
}

#[test]
fn page_byte_limit_keeps_the_single_entry_exception() {
    let mut graph = Graph::default();
    let mut oversized = file("a");
    oversized["entry_id"] = "x".repeat(NAMESPACE_PAGE_TARGET_BYTES).into();
    let single = graph.page(vec![oversized.clone()]);
    assert_eq!(graph.verify(single, 1, 1).unwrap(), 1);
    let multiple = graph.page(vec![oversized, file("b")]);
    rejects(
        graph.verify(multiple, 2, 2),
        ErrorCode::ObjectCorrupt,
        "namespace page exceeds the byte bound",
    );
}

#[test]
fn authenticated_wire_shape_retains_the_json_error_source() {
    for field in ["lookup_key", "size", "unexpected"] {
        let mut graph = Graph::default();
        let mut entry = file("a");
        entry[field] = "invalid".into();
        let root = graph.page(vec![entry]);
        let error = graph.verify(root, 1, 1).unwrap_err();
        assert_eq!(
            error.code(),
            ErrorCode::InvalidJson,
            "field={field}: {error:?}"
        );
        assert!(std::error::Error::source(&error).is_some(), "{error:?}");
    }
}

#[test]
fn altered_bytes_fail_authentication_instead_of_semantic_admission() {
    let mut graph = Graph::default();
    let root = graph.page(vec![file("a")]);
    graph.bytes.insert(
        (NAMESPACE_PAGE_KIND, root.hash.clone()),
        canonical_json(&json_codec::json!({"format":"namespace-page", "entries":[]})).unwrap(),
    );
    rejects(
        graph.verify(root, 1, 1),
        ErrorCode::ObjectCorrupt,
        "immutable object digest differs",
    );
}

#[test]
fn first_key_and_index_totals_must_match_the_rehashed_child_graph() {
    for mutation in 0..3 {
        let mut graph = Graph::default();
        let mut children = two_pages(&mut graph);
        let message = match mutation {
            0 => {
                children[1].first_key = "z".into();
                "namespace index first key differs from its subgraph"
            }
            1 => {
                children[1].entries = u64::MAX;
                "namespace index totals differ from its subgraph"
            }
            2 => {
                children[1].files = u64::MAX;
                "namespace index totals differ from its subgraph"
            }
            _ => unreachable!(),
        };
        let root = graph.index(children);
        rejects(graph.verify(root, 2, 2), ErrorCode::ObjectCorrupt, message);
        assert_eq!(graph.reads.get(), 3);
    }
}

#[test]
fn index_shape_rejects_empty_children_unordered_keys_and_excess_fanout() {
    for mutation in 0..4 {
        let mut graph = Graph::default();
        let mut children = two_pages(&mut graph);
        let message = match mutation {
            0 => {
                children.clear();
                "namespace index fanout is out of bounds"
            }
            1 => {
                children[0].entries = 0;
                "namespace index references an empty child"
            }
            2 => {
                children.swap(0, 1);
                "namespace index keys are not strictly increasing"
            }
            3 => {
                children = (0..129)
                    .map(|n| child(&format!("k{n:03}"), children[0].reference.clone(), 1))
                    .collect();
                "namespace index fanout is out of bounds"
            }
            _ => unreachable!(),
        };
        let root = graph.index(children);
        rejects(graph.verify(root, 2, 2), ErrorCode::ObjectCorrupt, message);
        assert_eq!(graph.reads.get(), 1);
    }
}

#[test]
fn index_must_not_disguise_an_empty_page_as_a_nonempty_child() {
    let mut graph = Graph::default();
    let empty = graph.page(vec![]);
    let right = graph.page(vec![file("b")]);
    let root = graph.index(vec![child("a", empty, 1), child("b", right, 1)]);
    rejects(
        graph.verify(root, 2, 2),
        ErrorCode::ObjectCorrupt,
        "a namespace index references an empty page",
    );
    assert_eq!(graph.reads.get(), 2);
}

#[test]
fn committed_minimal_page_corpus_uses_the_same_authenticated_entry() {
    let value = decode_strict(include_bytes!(
        "../../../fixtures/metadata_adversarial/namespace/unsorted-page.json"
    ))
    .unwrap();
    let mut graph = Graph::default();
    let root = graph.add(NamespaceRefKind::Page, value);
    rejects(
        graph.verify(root, 2, 2),
        ErrorCode::ObjectCorrupt,
        "namespace page keys are not strictly increasing",
    );
}

#[test]
fn cross_page_overlap_is_rejected_even_when_each_page_is_sorted() {
    let mut graph = Graph::default();
    let left = graph.page(vec![file("a"), file("c")]);
    let right = graph.page(vec![file("b")]);
    let root = graph.index(vec![child("a", left, 2), child("b", right, 1)]);
    rejects(
        graph.verify(root, 3, 3),
        ErrorCode::ObjectCorrupt,
        "namespace directory keys are not unique and sorted",
    );
}

#[test]
fn directory_and_commit_recorded_totals_are_never_allocation_lengths() {
    let mut graph = Graph::default();
    let leaf = graph.page(vec![file("a")]);
    rejects(
        graph.verify(leaf.clone(), u64::MAX, 1),
        ErrorCode::ObjectCorrupt,
        "namespace recorded entry total differs from the verified graph",
    );
    assert_eq!(graph.reads.get(), 1);
    assert!(graph.read_bytes.get() < 1024);
    let root = graph.page(vec![json_codec::json!({
        "entry_id":"entry_d", "name":"d", "lookup_key":"d", "kind":"directory",
        "entries":u64::MAX.to_string(), "files":"1", "subtree":leaf
    })]);
    rejects(
        graph.verify(root, 2, 2),
        ErrorCode::ObjectCorrupt,
        "namespace recorded totals differ from the verified subtree",
    );
}

#[test]
fn zero_entry_budget_stops_before_any_index_child_read() {
    let mut graph = Graph::default();
    let children = two_pages(&mut graph);
    let root = graph.index(children);
    rejects(
        graph.verify(root, u64::MAX, 0),
        ErrorCode::StateInvalid,
        "namespace exceeds the entry limit",
    );
    assert_eq!(graph.reads.get(), 1);
}

#[test]
fn entry_budget_stops_before_the_next_directory_subtree_read() {
    let mut graph = Graph::default();
    let subtree = graph.page(vec![file("a")]);
    let root = graph.page(vec![json_codec::json!({
        "entry_id":"entry_d", "name":"d", "lookup_key":"d", "kind":"directory",
        "entries":"1", "files":"1", "subtree":subtree
    })]);
    rejects(
        graph.verify(root, 2, 0),
        ErrorCode::StateInvalid,
        "namespace exceeds the entry limit",
    );
    assert_eq!(graph.reads.get(), 1);
}

fn chain(graph: &mut Graph, name: &str, indexes: usize) -> NamespaceIndexChild {
    let leaf = graph.page(vec![file(name)]);
    let mut current = child(name, leaf, 1);
    for _ in 0..indexes {
        let reference = graph.index(vec![current]);
        current = child(name, reference, 1);
    }
    current
}

#[test]
fn nonroot_unary_indexes_are_valid_but_unequal_leaf_depths_are_not() {
    let mut graph = Graph::default();
    let a = chain(&mut graph, "a", 1);
    let b = chain(&mut graph, "b", 1);
    let balanced = graph.index(vec![a.clone(), b]);
    assert_eq!(graph.verify(balanced, 2, 2).unwrap(), 2);
    let short = chain(&mut graph, "b", 0);
    let unbalanced = graph.index(vec![a, short]);
    rejects(
        graph.verify(unbalanced, 2, 2),
        ErrorCode::ObjectCorrupt,
        "namespace index children have different heights",
    );
}

#[test]
fn index_depth_bound_rejects_before_loading_the_fifth_object() {
    let mut graph = Graph::default();
    let a = chain(&mut graph, "a", 3);
    let b = chain(&mut graph, "b", 3);
    let root = graph.index(vec![a, b]);
    rejects(
        graph.verify(root, 2, 2),
        ErrorCode::ObjectCorrupt,
        "namespace index exceeds the depth bound",
    );
    assert_eq!(graph.reads.get(), 4);
}

#[test]
fn composed_directory_depth_keeps_the_path_error_source() {
    let mut graph = Graph::default();
    let mut root = graph.page(vec![file("a")]);
    let mut count = 1;
    for _ in 0..32 {
        root = graph.page(vec![json_codec::json!({
            "entry_id":format!("entry_d{count}"), "name":"d", "lookup_key":"d", "kind":"directory",
            "entries":count.to_string(), "files":"1", "subtree":root
        })]);
        count += 1;
    }
    let error = graph.verify(root, count, count).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    assert_eq!(
        error.message(),
        "namespace path exceeds the portable path budget"
    );
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(graph.reads.get(), 33);
}

#[test]
fn fixed_seed_combinations_replay_without_using_admission_as_an_oracle() {
    let mut seed = 0x4d45_5441_2026_1029u64;
    for case in 0..64 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let mutation = seed % 4;
        let mut graph = Graph::default();
        let mut entries = vec![file("a"), file("b"), file("c")];
        // Expected outcomes follow the explicit edit, before calling production code.
        let expected = match mutation {
            0 => None,
            1 => {
                entries.swap(0, 2);
                Some("namespace page keys are not strictly increasing")
            }
            2 => {
                entries[1] = file("A");
                Some("namespace page keys are not strictly increasing")
            }
            3 => Some("namespace recorded entry total differs from the verified graph"),
            _ => unreachable!(),
        };
        let root = graph.page(entries);
        let result = graph.verify(root, if mutation == 3 { u64::MAX } else { 3 }, 3);
        eprintln!(
            "namespace seed=0x4d45544120261029 case={case} state={seed:016x} mutation={mutation}"
        );
        match expected {
            None => assert_eq!(result.unwrap(), 3),
            Some(message) => rejects(result, ErrorCode::ObjectCorrupt, message),
        }
        assert_eq!(graph.reads.get(), 1);
        assert!(graph.read_bytes.get() < 2048);
    }
}
