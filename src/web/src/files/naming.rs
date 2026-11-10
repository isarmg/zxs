use web_interface::i18n::t;

#[derive(Debug, PartialEq, Eq, error_definition::Error)]
#[error("{name:?}: {source}")]
pub(super) struct Error {
    pub(super) name: String,
    #[source]
    pub(super) source: name_policy::Error,
}

impl Error {
    pub(super) fn new(name: &str, source: name_policy::Error) -> Self {
        Self {
            name: name.into(),
            source,
        }
    }

    pub(super) fn candidate(&self) -> Option<&str> {
        match &self.source {
            name_policy::Error::NotNormalized(candidate) => Some(candidate),
            _ => None,
        }
    }

    pub(super) fn message(&self) -> String {
        let reason = match &self.source {
            name_policy::Error::EmptyOrDot => t(
                "名称不能为空、. 或 ..。",
                "Names cannot be empty, '.' or '..'.",
            ),
            name_policy::Error::NameTooLong => t(
                "名称超过 240 字节或 UTF-16 代码单元。",
                "The name exceeds 240 bytes or UTF-16 units.",
            ),
            name_policy::Error::NameEdge => t(
                "名称不能以空格开头或以空格、句点结尾。",
                "Names cannot start with a space or end with a space or period.",
            ),
            name_policy::Error::UnsupportedCharacter(code) => {
                let position = self
                    .name
                    .chars()
                    .position(|ch| u32::from(ch) == *code)
                    .expect("invariant: policy character belongs to the rejected input");
                format!(
                    "{} U+{code:04X} ({} {position})",
                    t("不支持的字符", "Unsupported character"),
                    t("从零起算的字符位置", "zero-based character position")
                )
            }
            name_policy::Error::ReservedName => {
                t("名称属于设备保留名。", "This is a reserved device name.")
            }
            name_policy::Error::NotNormalized(candidate) => format!(
                "{} {candidate:?}",
                t(
                    "NFC 候选名称（仅明确确认后使用）：",
                    "NFC candidate (used only after explicit confirmation):"
                )
            ),
            name_policy::Error::InvalidPath => t(
                "完整目标路径无效或超过长度预算。",
                "The complete destination path is invalid or exceeds its length budget.",
            ),
            name_policy::Error::PathTooDeep => {
                t("目录层级超过 32。", "The path exceeds 32 levels.")
            }
            name_policy::Error::InvalidSlot => t("盘位标识无效。", "Invalid disk slot identifier."),
        };
        format!(
            "{} {:?} — {reason} {}",
            t("原名称：", "Original name:"),
            self.name,
            t(
                "请修改输入或取消；不会自动改名。",
                "Edit the input or cancel; names are never changed automatically."
            )
        )
    }
}

pub(super) fn validate(name: &str) -> Result<(), Error> {
    name_policy::validate_name(name).map_err(|source| Error::new(name, source))
}
