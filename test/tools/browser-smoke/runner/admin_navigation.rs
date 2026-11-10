//! Real header navigation to the current responsibility-specific management pages.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdminPage {
    Storage,
    Repositories,
    Rules,
    Diagnostics,
}

impl AdminPage {
    pub(super) const ALL: [Self; 4] = [
        Self::Storage,
        Self::Repositories,
        Self::Rules,
        Self::Diagnostics,
    ];

    pub(super) fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Storage => ("Disks and admission", "磁盘与接入"),
            Self::Repositories => ("Repositories", "仓库"),
            Self::Rules => ("Backup and placement", "备份与放置"),
            Self::Diagnostics => ("Recovery and diagnostics", "恢复与检查"),
        }
    }

    pub(super) fn section(self) -> &'static str {
        match self {
            Self::Storage => "section[data-zxs-admin-section='storage']",
            Self::Repositories => "section[data-zxs-admin-section='repositories']",
            Self::Rules => "section[data-zxs-admin-section='rules']",
            Self::Diagnostics => "section[data-zxs-admin-section='diagnostics']",
        }
    }

    pub(super) fn open(self, browser: &Browser) -> Result<()> {
        let (en, zh) = self.labels();
        let button = browser.element("xpath", &format!(
            "//nav[contains(concat(' ',normalize-space(@class),' '),' zxs-header-navigation ')]//button[@data-zxs-management-menu='true' and (normalize-space(.)='{en}' or normalize-space(.)='{zh}')]"
        ))?;
        browser.click(&button)?;
        browser.css(self.section())?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../unit/admin_navigation_tests.rs"]
mod tests;
