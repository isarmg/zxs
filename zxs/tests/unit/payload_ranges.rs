use super::*;

const TARGET: &str = "/zxs/v1/containers/11111111111111111111111111111111/nodes/22222222222222222222222222222222/payload";
const SHA: [u8; 32] = [0xab; 32];

fn selected(headers: &[(&str, &str)]) -> PayloadRead {
    let Request::NodePayload(read) = payload_request("GET", TARGET, headers).unwrap() else {
        panic!("not the distinct payload request");
    };
    read
}

#[test]
fn distinct_bearer_path_does_not_relax_old_r3_data_route() {
    assert!(is_payload_target(TARGET));
    assert!(!is_payload_target(&format!("{TARGET}?legacy=true")));
    assert!(!is_payload_target(&format!("{TARGET}/")));
    let legacy = TARGET.replace("/payload", "/data");
    assert!(!is_payload_target(&legacy));
    assert!(matches!(
        super::super::request("GET", &legacy, &[]),
        Ok(Request::NodeData { .. })
    ));
    assert_eq!(
        super::super::request("GET", &legacy, &[("Range", "bytes=0-1")]),
        Err(Rejected::Header)
    );
    assert_eq!(
        super::super::request("GET", TARGET, &[]),
        Err(Rejected::Route)
    );
    assert_eq!(payload_request("HEAD", TARGET, &[]), Err(Rejected::Route));
    assert!(payload_request("GET", &TARGET.replace("1111", "zzzz"), &[]).is_err());
    assert!(payload_request("GET", TARGET, &[("Content-Length", "1")]).is_err());
}

#[test]
fn closed_open_ended_suffix_ranges_clamp_and_use_checked_full_width() {
    for (raw, total, start, end) in [
        ("bytes=2-5", 17, 2, 5),
        ("bytes=3-", 17, 3, 16),
        ("bytes=3-99", 17, 3, 16),
        ("bytes=-4", 17, 13, 16),
        ("Bytes=-90", 17, 0, 16),
        ("bytes=-65557", 130_000, 64_443, 129_999),
        ("bytes=0-", u64::MAX, 0, u64::MAX - 1),
    ] {
        let PayloadWindow::Span(span) = selected(&[("Range", raw)]).window(total, SHA) else {
            panic!("valid range was refused");
        };
        assert_eq!(span.bounds(), (start, end));
        assert_eq!(span.length(), end - start + 1);
    }
    for (raw, length) in [("bytes=-0", 1), ("bytes=0-", 0), ("bytes=2-3", 2)] {
        assert_eq!(
            selected(&[("Range", raw)]).window(length, SHA),
            PayloadWindow::Unsatisfiable
        );
    }
}

#[test]
fn no_ambiguous_range_can_fall_back_to_payload_bytes() {
    for header in [
        "bytes=0-1,4-5",
        "bytes=5-4",
        "bytes=-",
        "bytes=18446744073709551616-",
        "bytes=0 - 1",
        "bytes=1-2-3",
        "bytes=",
        "bytes0-1",
    ] {
        assert!(payload_request("GET", TARGET, &[("Range", header)]).is_err());
    }
    assert_eq!(
        payload_request(
            "GET",
            TARGET,
            &[("Range", "bytes=0-1"), ("rAnGe", "bytes=0-1")]
        ),
        Err(Rejected::Header)
    );
    assert_eq!(
        payload_request("GET", TARGET, &[("If-Range", "a"), ("if-range", "b")]),
        Err(Rejected::Header)
    );
    assert_eq!(
        selected(&[("Range", "objects=a-b")]).window(45, SHA),
        PayloadWindow::Full
    );
}

#[test]
fn only_the_exact_strong_whole_validator_authorizes_a_partial_reply() {
    let tag = format!("\"{}\"", "ab".repeat(32));
    let range = ("Range", "bytes=3-4");
    let eligible = selected(&[range, ("If-Range", &tag)]);
    assert!(matches!(eligible.window(19, SHA), PayloadWindow::Span(_)));
    assert_eq!(eligible.window(19, [3; 32]), PayloadWindow::Full);
    for mismatch in [
        format!("W/{tag}"),
        format!("\"{}\"", "AB".repeat(32)),
        "Wed, 21 Oct 2015 07:28:00 GMT".to_owned(),
    ] {
        assert_eq!(
            selected(&[range, ("If-Range", &mismatch)]).window(19, SHA),
            PayloadWindow::Full
        );
    }
}
