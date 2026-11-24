use super::*;
use crate::r3::{Digest, DisplayName, Head, Node};
use http::Method;
use serde_json::{json, to_vec};

fn id(n: u16) -> Id {
    Id::parse(&format!("{n:032x}")).unwrap()
}

#[test]
fn browse_credential_admits_only_exact_typed_gets() {
    let c = id(1);
    let head = id(2);
    let after = id(3);
    let branch = Branch::parse("main").unwrap();
    let browse = ReadPurpose::Browse;
    for typed in [
        Metadata::Catalog(Some(&after)),
        Metadata::Detail {
            container: &c,
            sequence: Some(3),
            after: Some(&branch),
        },
        Metadata::History {
            container: &c,
            branch: &branch,
            head: &head,
            next: Some(&after),
        },
        Metadata::Tags(&c),
    ] {
        let out = typed.build().unwrap();
        browse.require_route(&out).unwrap();
        assert_eq!(out.method(), Method::GET);
        assert!(!out.headers().contains_key(header::CONTENT_LENGTH));
        assert!(!out.headers().contains_key(header::CONTENT_TYPE));
        assert!(ReadPurpose::Receipt.require_route(&out).is_err());
        assert!(ReadPurpose::Payload.require_route(&out).is_err());
    }

    for path in [
        format!("{ROOT}?after={after}"),
        format!("{ROOT}/{c}/nodes/{head}/data"),
        format!("{ROOT}/{c}/nodes/{head}/payload"),
        format!("{ROOT}/{c}/operations/{after}"),
        format!("{ROOT}/{c}/branches/{branch}/nodes/"),
        format!("https://unexpected.example{ROOT}/{c}/tags"),
        "/zxs/v1/uploads".to_owned(),
    ] {
        assert!(
            browse
                .require_route(&r3::json_request(path).unwrap())
                .is_err()
        );
    }

    let mut bad = Metadata::Tags(&c).build().unwrap();
    *bad.method_mut() = Method::PUT;
    assert!(browse.require_route(&bad).is_err());
    let mut bad = Metadata::Tags(&c).build().unwrap();
    bad.headers_mut()
        .insert(header::RANGE, HeaderValue::from_static("bytes=0-1"));
    assert!(browse.require_route(&bad).is_err());
    let mut bad = Metadata::Catalog(Some(&after)).build().unwrap();
    bad.headers_mut()
        .append("zxs-after", HeaderValue::from_str(after.as_str()).unwrap());
    assert!(browse.require_route(&bad).is_err());
    let mut bad = Metadata::Detail {
        container: &c,
        sequence: Some(3),
        after: Some(&branch),
    }
    .build()
    .unwrap();
    bad.headers_mut()
        .insert("zxs-detail-sequence", HeaderValue::from_static("03"));
    assert!(browse.require_route(&bad).is_err());
    assert!(
        Metadata::Detail {
            container: &c,
            sequence: None,
            after: Some(&branch)
        }
        .build()
        .is_err()
    );
    assert!(
        Metadata::History {
            container: &c,
            branch: &branch,
            head: &head,
            next: Some(&head)
        }
        .build()
        .is_err()
    );
}

#[test]
fn browse_sorted_pages_keep_exact_original_after_and_revision() {
    let c = id(1);
    let name = DisplayName::parse("ordinary.bin").unwrap();
    let catalog =
        json!({"items":[{"container_id": c,"sequence":4,"name": name}],"next_after":null});
    assert_eq!(
        catalog_page(&to_vec(&catalog).unwrap(), None)
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(catalog_page(&to_vec(&catalog).unwrap(), Some(&c)).is_err());
    assert!(catalog_page(br#"{"items":[]}"#, None).is_err());
    assert!(catalog_page(br#"{"items":[],"next_after":null,"items":[]}"#, None).is_err());
    let main = Branch::parse("main").unwrap();
    let detail = Detail {
        container_id: c.clone(),
        sequence: 9,
        name,
        tags: vec!["exact".to_owned()],
        heads: vec![Head {
            branch: main.clone(),
            head: id(2),
        }],
        next_after: None,
    };
    let raw = to_vec(&detail).unwrap();
    assert_eq!(detail_page(&raw, &c, Some(9), None).unwrap(), detail);
    assert!(detail_page(&raw, &c, Some(8), None).is_err());
    assert!(detail_page(&raw, &id(2), Some(9), None).is_err());
    assert!(detail_page(&raw, &c, Some(9), Some(&main)).is_err());
    let items: Vec<_> = (1..=256_u16)
        .map(|n| {
            json!({
                "container_id":id(n),"sequence":n,"name":"section"
            })
        })
        .collect();
    assert!(
        catalog_page(
            &to_vec(&json!({"items":items,"next_after":id(256)})).unwrap(),
            None
        )
        .is_ok()
    );
    assert!(
        catalog_page(
            &to_vec(&json!({"items":items,"next_after":id(257)})).unwrap(),
            None
        )
        .is_err()
    );
}

fn node(id_n: u16, parent: Option<u16>, sequence: u64) -> Node {
    Node {
        node_id: id(id_n),
        operation_id: id(id_n),
        parent_node: parent.map(id),
        original_branch: Branch::parse("main").unwrap(),
        sequence,
        length: 12,
        sha256: Digest::parse(&"4".repeat(64)).unwrap(),
    }
}

#[test]
fn browse_history_uses_first_unreturned_ancestor_not_last_returned_id() {
    let head = id(1);
    let mut page = History {
        head: head.clone(),
        nodes: (1..=256_u16)
            .map(|n| node(n, Some(n + 1), u64::from(258 - n)))
            .collect(),
        next: Some(id(257)),
    };
    assert!(history_page(&to_vec(&page).unwrap(), &head, None).is_ok());
    page.next = Some(id(256));
    assert!(history_page(&to_vec(&page).unwrap(), &head, None).is_err());
    page.next = Some(id(257));
    page.nodes[1].sequence = page.nodes[0].sequence;
    assert!(history_page(&to_vec(&page).unwrap(), &head, None).is_err());

    let last = History {
        head: head.clone(),
        nodes: vec![node(257, None, 1)],
        next: None,
    };
    assert!(history_page(&to_vec(&last).unwrap(), &head, Some(&id(257))).is_ok());
    assert!(history_page(&to_vec(&last).unwrap(), &head, Some(&id(258))).is_err());
    let mut missing = serde_json::to_value(last).unwrap();
    missing["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("parent_node");
    assert!(history_page(&to_vec(&missing).unwrap(), &head, Some(&id(257))).is_err());
}

#[test]
fn browse_tags_and_empty_outcomes_never_substitute_success() {
    assert_eq!(
        tags_reply(br#"["second","first","second"]"#).unwrap(),
        ["second", "first", "second"]
    );
    assert!(tags_reply(br#"{"tags":[]}"#).is_err());
    assert!(tags_reply(br#"[""]"#).is_err());
    assert!(tags_reply(br#"["\u0000"]"#).is_err());

    let mut zero = HeaderMap::new();
    zero.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    zero.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    assert_eq!(
        reply_status(StatusCode::NOT_FOUND, &zero).unwrap(),
        Some(BrowseError::Missing)
    );
    assert_eq!(
        reply_status(StatusCode::CONFLICT, &zero).unwrap(),
        Some(BrowseError::SelectionChanged)
    );
    assert_eq!(
        reply_status(StatusCode::SERVICE_UNAVAILABLE, &zero).unwrap(),
        Some(BrowseError::Unavailable)
    );
    assert!(reply_status(StatusCode::PARTIAL_CONTENT, &zero).is_err());
    zero.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    assert!(reply_status(StatusCode::NOT_FOUND, &zero).is_err());
}
