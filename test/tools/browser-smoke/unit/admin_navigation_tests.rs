use super::*;

#[test]
fn administration_navigation_has_four_distinct_current_destinations() {
    let expected = [
        (("Disks and admission", "磁盘与接入"), "storage"),
        (("Repositories", "仓库"), "repositories"),
        (("Backup and placement", "备份与放置"), "rules"),
        (("Recovery and diagnostics", "恢复与检查"), "diagnostics"),
    ];
    for (page, (labels, section)) in AdminPage::ALL.into_iter().zip(expected) {
        assert_eq!(page.labels(), labels);
        assert_eq!(
            page.section(),
            format!("section[data-zxs-admin-section='{section}']")
        );
    }
}
