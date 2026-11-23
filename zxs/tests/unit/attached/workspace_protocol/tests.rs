use super::*;

const CONT: &str = "01010101010101010101010101010101";
const OTHER: &str = "04040404040404040404040404040404";
const OP: &str = "02020202020202020202020202020202";
const NODE: &str = "03030303030303030303030303030303";
const OLD: &str = "05050505050505050505050505050505";
const FORK: &str = "06060606060606060606060606060606";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn full() -> [(&'static str, &'static str); 5] {
    [
        ("Content-Length", "4"),
        ("Content-Type", "application/octet-stream"),
        ("Zxs-Operation", OP),
        ("Zxs-Node", NODE),
        ("Zxs-Sha256", SHA),
    ]
}
fn parsed(branch: &str, extra: &[(&str, &str)]) -> Result<Request, Rejected> {
    let mut fields = full().to_vec();
    fields.extend(extra);
    request(
        "POST",
        &format!("{ROOT}/{CONT}/branches/{branch}/nodes"),
        &fields,
    )
}
fn id(byte: u8) -> Id {
    Id([byte; 16])
}
fn expected(target: Target) -> Request {
    Request::SaveNode(SaveNode {
        container: id(1),
        operation: id(2),
        node: id(3),
        target,
        length: 4,
        sha256: [0xaa; 32],
    })
}

#[test]
fn first_main_is_one_opaque_save_with_exact_name_node_and_operation() {
    let r = parsed(
        "main",
        &[("Zxs-Base-Node", "0"), ("Zxs-Initial-Name", "e696b0")],
    );
    assert_eq!(
        r,
        Ok(expected(Target::FirstMain {
            name: String::from("新")
        }))
    );
    assert!(parsed("main", &[("Zxs-Base-Node", "0")]).is_err());
    assert!(parsed(FORK, &[("Zxs-Base-Node", "0"), ("Zxs-Initial-Name", "61")]).is_err());
    assert!(
        parsed(
            "main",
            &[("Zxs-Base-Node", "0"), ("Zxs-Initial-Name", "2f")]
        )
        .is_err()
    );
}

#[test]
fn append_current_uses_exact_requested_head_not_generation_or_filename() {
    assert_eq!(
        parsed("main", &[("Zxs-Base-Node", OLD)]),
        Ok(expected(Target::Branch {
            branch: Branch::Main,
            expected_head: id(5)
        }))
    );
    assert_eq!(
        parsed(FORK, &[("Zxs-Base-Node", OLD)]),
        Ok(expected(Target::Branch {
            branch: Branch::Named(id(6)),
            expected_head: id(5)
        }))
    );
    assert!(parsed("main", &[("Zxs-Base-Generation", "1")]).is_err());
}

#[test]
fn historical_main_is_explicitly_a_new_branch_with_the_same_whole_body() {
    assert_eq!(
        parsed(FORK, &[("Zxs-Fork-From-Main", OLD)]),
        Ok(expected(Target::HistoricalMain {
            new_branch: id(6),
            original: id(5)
        }))
    );
    assert!(parsed("main", &[("Zxs-Fork-From-Main", OLD)]).is_err());
}

#[test]
fn mixed_duplicate_or_partial_save_inputs_do_not_decode() {
    for invalid in [
        [("Zxs-Base-Node", OLD), ("Zxs-Fork-From-Main", OLD)],
        [("Zxs-Base-Node", OLD), ("Zxs-Initial-Name", "61")],
        [("Zxs-Base-Node", OLD), ("zxs-base-node", OLD)],
        [("Zxs-Base-Node", OLD), ("Content-Length", "4")],
        [("Zxs-Base-Node", OLD), ("Content-Range", "bytes 0-3/8")],
        [("Zxs-Base-Node", OLD), ("Transfer-Encoding", "chunked")],
    ] {
        assert!(parsed(FORK, &invalid).is_err());
    }
}

#[test]
fn body_is_exactly_one_opaque_extent_not_workspace_archive_or_delta() {
    use std::io::Read as _;
    let Request::SaveNode(save) = parsed("main", &[("Zxs-Base-Node", OLD)]).unwrap() else {
        panic!("expected one container save");
    };
    let mut parts = &b"\x00\xff\x01\x7ftail"[..];
    let mut body = save.body(&mut parts);
    let mut out = Vec::new();
    body.read_to_end(&mut out).unwrap();
    assert_eq!(body.limit(), 0);
    assert_eq!(out, [0, 255, 1, 127]);
    assert_eq!(parts, b"tail");
    let mut short = save.body(&b"abc"[..]);
    short.read_to_end(&mut Vec::new()).unwrap();
    assert_eq!(short.limit(), 1); // Store compares total length before any selection.
    assert!(request("POST", "/zxs/v1/workspace/archive", &full()).is_err());
    assert!(request("PUT", &format!("{ROOT}/{CONT}/delta"), &full()).is_err());
}

#[test]
fn deletion_is_an_exact_recycle_request_and_never_a_purge_route() {
    let fields = [("Zxs-Operation", OP), ("Zxs-Base-Generation", "11")];
    assert_eq!(
        request("DELETE", &format!("{ROOT}/{CONT}"), &fields),
        Ok(Request::Delete(MetadataMutation {
            container: id(1),
            operation: id(2),
            revision: 11
        }))
    );
    assert!(request("DELETE", &format!("{ROOT}/{CONT}/purge"), &fields).is_err());
    let invalid = [
        ("Zxs-Operation", OP),
        ("Zxs-Base-Generation", "11"),
        ("Content-Length", "1"),
    ];
    assert!(request("DELETE", &format!("{ROOT}/{CONT}"), &invalid).is_err());
    // Decoding DELETE never implies the disk transfer or its receipt happened.
    assert_eq!(
        request("GET", &format!("{ROOT}/{CONT}/operations/{OP}"), &[]),
        Ok(Request::Receipt {
            container: id(1),
            operation: id(2)
        })
    );
    assert_eq!(
        request("GET", &format!("{ROOT}/{OTHER}/operations/{OP}"), &[]),
        Ok(Request::Receipt {
            container: id(4),
            operation: id(2)
        })
    );
}

#[test]
fn initial_name_uses_the_storage_display_rule_before_body_intake() {
    for invalid in ["612e", "6120", "613a62", "613c62", "610062", "ff"] {
        assert_eq!(
            parsed(
                "main",
                &[("Zxs-Base-Node", "0"), ("Zxs-Initial-Name", invalid)]
            ),
            Err(Rejected::Value)
        );
    }
    assert!(name_hex(&"61".repeat(255)).is_ok());
    assert!(matches!(name_hex(&"61".repeat(256)), Err(Rejected::Value)));
}

#[test]
fn compact_catalog_accepts_only_singleton_collection_headers() {
    assert_eq!(request("GET", ROOT, &[]), Ok(Request::List));
    assert_eq!(
        request("GET", ROOT, &[("Zxs-View", "compact")]),
        Ok(Request::ListCompact { after: None })
    );
    assert_eq!(
        request("GET", ROOT, &[("zxs-VIEW", "compact"), ("Zxs-After", CONT)],),
        Ok(Request::ListCompact { after: Some(id(1)) })
    );
    for fields in [
        vec![("Zxs-After", CONT)],
        vec![("Zxs-View", "Compact")],
        vec![("Zxs-View", "compact"), ("zxs-view", "compact")],
        vec![
            ("Zxs-View", "compact"),
            ("Zxs-After", CONT),
            ("zxs-after", OTHER),
        ],
        vec![
            ("Zxs-View", "compact"),
            ("Zxs-After", "00000000000000000000000000000000"),
        ],
        vec![
            ("Zxs-View", "compact"),
            ("Zxs-After", "A1010101010101010101010101010101"),
        ],
        vec![("Zxs-View", "compact"), ("Zxs-After", "01")],
        vec![("Zxs-View", "compact"), ("Content-Length", "1")],
    ] {
        assert!(request("GET", ROOT, &fields).is_err());
    }
    let detail = format!("{ROOT}/{CONT}");
    assert!(request("GET", &detail, &[("Zxs-View", "compact")]).is_err());
    assert!(request("GET", &detail, &[("Zxs-After", CONT)]).is_err());
    assert!(request("POST", ROOT, &[("Zxs-View", "compact")]).is_err());
}

#[test]
fn history_page_headers_require_the_exact_branch_read_and_a_distinct_parent_cursor() {
    let path = format!("{ROOT}/{CONT}/branches/main/nodes");
    assert_eq!(
        request("GET", &path, &[]),
        Ok(Request::BranchNodes {
            container: id(1),
            branch: Branch::Main,
        })
    );
    assert_eq!(
        request("GET", &path, &[("zxs-history-head", NODE)]),
        Ok(Request::BranchHistory {
            container: id(1),
            branch: Branch::Main,
            head: id(3),
            next: None,
        })
    );
    let continued = [("Zxs-History-Head", NODE), ("Zxs-History-Next", OLD)];
    assert_eq!(
        request("GET", &path, &continued),
        Ok(Request::BranchHistory {
            container: id(1),
            branch: Branch::Main,
            head: id(3),
            next: Some(id(5)),
        })
    );
    for invalid in [
        vec![("zxs-history-next", OLD)],
        vec![("zxs-history-head", NODE), ("zxs-history-next", NODE)],
        vec![("zxs-history-head", NODE), ("Zxs-History-Head", NODE)],
        vec![
            ("zxs-history-head", NODE),
            ("zxs-history-next", OLD),
            ("Zxs-History-Next", OLD),
        ],
        vec![("zxs-history-head", "00000000000000000000000000000000")],
        vec![("zxs-history-head", "AA030303030303030303030303030303")],
        vec![("zxs-history-head", NODE), ("Content-Length", "1")],
        vec![("zxs-history-head", NODE), ("zxs-view", "compact")],
    ] {
        assert!(request("GET", &path, &invalid).is_err());
    }
    assert!(request("GET", ROOT, &continued).is_err());
    assert!(request("POST", &path, &continued).is_err());
}

#[test]
fn paged_detail_refuses_unbound_or_noncanonical_request_fields() {
    let target = format!("{ROOT}/{CONT}");
    assert_eq!(request("GET", &target, &[]), Ok(Request::Container(id(1))));
    assert_eq!(
        request("GET", &target, &[("Zxs-Detail", "paged")]),
        Ok(Request::ContainerDetail {
            container: id(1),
            sequence: None,
            after: None
        })
    );
    assert_eq!(
        request(
            "GET",
            &target,
            &[
                ("Zxs-Detail", "paged"),
                ("Zxs-Detail-Sequence", "7"),
                ("Zxs-Detail-After", "main")
            ]
        ),
        Ok(Request::ContainerDetail {
            container: id(1),
            sequence: Some(7),
            after: Some(Branch::Main)
        })
    );
    for bad in [
        vec![("Zxs-Detail-Sequence", "7")],
        vec![("Zxs-Detail", "paged"), ("Zxs-Detail-After", "main")],
        vec![("Zxs-Detail", "paged"), ("Zxs-Detail-Sequence", "0")],
        vec![("Zxs-Detail", "paged"), ("Zxs-Detail-Sequence", "07")],
        vec![("Zxs-Detail", "paged"), ("zxs-detail", "paged")],
        vec![
            ("Zxs-Detail", "paged"),
            ("Zxs-Detail-Sequence", "7"),
            ("Zxs-Detail-After", "MAIN"),
        ],
    ] {
        assert!(request("GET", &target, &bad).is_err());
    }
    assert!(request("GET", ROOT, &[("Zxs-Detail", "paged")]).is_err());
    assert!(request("GET", &format!("{target}/tags"), &[("Zxs-Detail", "paged")]).is_err());
    assert!(request("DELETE", &target, &[("Zxs-Detail", "paged")]).is_err());
}
