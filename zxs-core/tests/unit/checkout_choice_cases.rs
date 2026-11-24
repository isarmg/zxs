use super::{OriginalHead, require_same_session};
use crate::r3::{Branch, Detail, Digest, DisplayName, Head, Id, Node};
use std::sync::Arc;

fn id(hexdigit: &str) -> Id {
    Id::parse(&hexdigit.repeat(32)).unwrap()
}

#[test]
fn original_checkout_rejects_changed_head_revision_or_node_identity() {
    let container = id("1");
    let branch = Branch::parse(id("2").as_str()).unwrap();
    let node = Node {
        node_id: id("3"),
        operation_id: id("4"),
        parent_node: Some(id("5")),
        // This branch legitimately holds a historical main node.
        original_branch: Branch::parse("main").unwrap(),
        sequence: 7,
        length: 12,
        sha256: Digest::parse(&"6".repeat(64)).unwrap(),
    };
    let detail = Detail {
        container_id: container.clone(),
        name: DisplayName::parse("ordinary.bin").unwrap(),
        sequence: 9,
        tags: vec![],
        heads: vec![Head {
            branch: branch.clone(),
            head: node.node_id.clone(),
        }],
        next_after: None,
    };
    let original = OriginalHead::new(&container, &branch, &detail, node.clone());
    original.require_node(&detail, &node).unwrap();
    assert_eq!(original.basis.body, node.body());
    assert_ne!(original.basis.branch, node.original_branch);

    for changed in [
        Detail {
            container_id: id("7"),
            ..detail.clone()
        },
        Detail {
            sequence: 10,
            ..detail.clone()
        },
        Detail {
            heads: vec![Head {
                branch: branch.clone(),
                head: id("8"),
            }],
            ..detail.clone()
        },
        Detail {
            heads: vec![Head {
                branch: node.original_branch.clone(),
                head: node.node_id.clone(),
            }],
            ..detail.clone()
        },
    ] {
        assert!(original.require_detail(&changed).is_err());
        assert!(original.require_node(&changed, &node).is_err());
    }
    assert!(
        original
            .require_node(
                &detail,
                &Node {
                    length: 13,
                    ..node.clone()
                }
            )
            .is_err()
    );
    assert!(
        original
            .require_node(
                &detail,
                &Node {
                    sha256: Digest::parse(&"9".repeat(64)).unwrap(),
                    ..node
                }
            )
            .is_err()
    );
}

#[test]
fn checkout_cannot_adopt_an_equivalent_but_fresh_session() {
    let selected = Arc::new(());
    require_same_session(&selected, &Arc::clone(&selected)).unwrap();
    assert!(require_same_session(&selected, &Arc::new(())).is_err());
}
