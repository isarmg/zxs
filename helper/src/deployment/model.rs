use super::{Input, SmbInput};
use crate::Result;
use data_serialization::Serialize;
use std::net::SocketAddr;

/// Deliberately narrow account token: usable unchanged in both systemd and
/// standalone Samba syntax, with no substitutions, comments or extra tokens.
#[derive(Serialize)]
pub(super) struct Account(String);
impl Account {
    fn new(value: String) -> Result<Self> {
        let first = value.as_bytes().first().copied();
        if value.is_empty()
            || value.len() > 32
            || value == "root"
            || !matches!(first, Some(b'a'..=b'z' | b'_'))
            || !value
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
        {
            return Err("account/group must be a non-root 1–32 byte lowercase Unix name, beginning with a letter or underscore; existence and numeric ownership are not verified by preparation".into());
        }
        Ok(Self(value))
    }
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Serialize)]
pub(super) struct Smb {
    pub account: Account,
    pub peer_uid: u32,
}
#[derive(Serialize)]
pub(super) struct Settings {
    pub user: Account,
    pub group: Account,
    pub listen: SocketAddr,
    pub smb: Option<Smb>,
}
impl TryFrom<Input> for Settings {
    type Error = Box<dyn std::error::Error + Send + Sync>;
    fn try_from(input: Input) -> Result<Self> {
        let user = Account::new(input.service_user)?;
        let group = Account::new(input.service_group)?;
        if input.listen.port() == 0 || input.listen.ip().is_multicast() {
            return Err("ZXS listen address must have a nonzero port and not be multicast".into());
        }
        if let SocketAddr::V6(address) = input.listen {
            if address.scope_id() != 0 || address.flowinfo() != 0 {
                return Err(
                    "scoped IPv6 addresses are not supported by this deployment format".into(),
                );
            }
        }
        let smb = match input.smb {
            Some(SmbInput { account, peer_uid }) => {
                let account = Account::new(account)?;
                if account.as_str() == user.as_str() {
                    return Err("Samba authentication account must be separate from the private ZXS service account".into());
                }
                Some(Smb { account, peer_uid })
            }
            None => None,
        };
        Ok(Self {
            user,
            group,
            listen: input.listen,
            smb,
        })
    }
}
