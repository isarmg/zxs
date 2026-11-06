use super::*;

#[test]
fn management_admission_probes_preserve_bytes_errors_and_cleanup() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/browser-smoke/unit/admin_read_queue_test.mjs");
    let result = Command::new("node")
        .arg(path)
        .output()
        .expect("Node is required by repository validation");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn administration_browser_expectations_cover_both_languages_without_mixed_labels() {
    assert_eq!(ACTIONS.len(), 9);
    assert_eq!(QUERIES.len(), 13);
    for pairs in [&ACTIONS[..], &QUERIES[..]] {
        let en = labels(pairs, "en");
        let zh = labels(pairs, "zh-CN");
        assert_eq!(en.len(), zh.len());
        for index in 0..pairs.len() {
            assert!(en[index].is_ascii());
            assert!(!zh[index].is_ascii());
            assert!(!en[..index].contains(&en[index]));
            assert!(!zh[..index].contains(&zh[index]));
        }
    }
}

#[test]
fn administration_controls_do_not_offer_a_second_smb_binding() {
    for pairs in [&ACTIONS[..], &QUERIES[..]] {
        assert!(
            pairs
                .iter()
                .all(|(en, zh)| !en.contains("SMB") && !zh.contains("SMB"))
        );
    }
}
