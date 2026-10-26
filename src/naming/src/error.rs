//! Portable-name parsing failures, independent of transport and storage.
#[derive(Debug, PartialEq, Eq, error_definition::Error)]
pub enum Error {
    #[error("名称不能为空或点路径")]
    EmptyOrDot,
    #[error("名称超过240字节或UTF-16代码单元")]
    NameTooLong,
    #[error("名称不能以空格开头或以空格、句点结尾")]
    NameEdge,
    #[error("名称包含不支持的字符 U+{0:04X}")]
    UnsupportedCharacter(u32),
    #[error("名称属于设备保留名")]
    ReservedName,
    #[error("名称须明确确认为NFC形式：{0}")]
    NotNormalized(String),
    #[error("路径不是允许的仓库相对路径或超过长度预算")]
    InvalidPath,
    #[error("目录层级超过32")]
    PathTooDeep,
    #[error("盘位标识须为字母开头的ASCII字母数字、下划线或连字符，最多64字节")]
    InvalidSlot,
}
