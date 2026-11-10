use web_interface::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiskRead {
    Current,
    Reading,
    RefreshingCached,
    Unconfirmed,
    UnconfirmedCached,
}

impl DiskRead {
    pub(super) fn message(self) -> Option<String> {
        match self {
            Self::Current => None,
            Self::Reading => Some(t("正在读取磁盘列表。", "Reading the disk list.")),
            Self::RefreshingCached => Some(t(
                "正在更新磁盘列表，当前显示上次读取结果。",
                "Updating the disk list. Showing the previous result.",
            )),
            Self::Unconfirmed => Some(t(
                "尚未取得磁盘列表，请重新读取。",
                "The disk list is not available yet. Read it again.",
            )),
            Self::UnconfirmedCached => Some(t(
                "磁盘列表尚未更新，当前显示上次读取结果。请重新读取以确认可用性。",
                "The disk list has not been refreshed. Showing the previous result; read it again to check availability.",
            )),
        }
    }

    pub(super) fn marker(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Reading => "reading",
            Self::RefreshingCached => "refreshing-cached",
            Self::Unconfirmed => "unconfirmed",
            Self::UnconfirmedCached => "unconfirmed-cached",
        }
    }

    pub(super) fn can_retry(self) -> bool {
        matches!(self, Self::Unconfirmed | Self::UnconfirmedCached)
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/view/disk_read_tests.rs"]
mod tests;
