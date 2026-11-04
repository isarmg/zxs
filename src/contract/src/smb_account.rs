//! SMB account presentation. Samba remains the credential authority.
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SmbAccountStatus {
    Disabled {},
    Configured {
        username: String,
        #[serialization(with = "crate::u64_string")]
        generation: u64,
        candidates: Vec<String>,
        password_change: PasswordChangeAvailability,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum PasswordChangeAvailability {
    Available,
    ClientUnavailable,
    UnsupportedAccountName,
    RequiresUnprivilegedService,
}
