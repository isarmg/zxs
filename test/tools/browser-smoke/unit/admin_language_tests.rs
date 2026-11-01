use super::*;

#[test]
fn administration_browser_expectations_cover_both_languages_without_mixed_labels() {
    assert_eq!(ACTIONS.len(), 10);
    assert_eq!(QUERIES.len(), 14);
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
