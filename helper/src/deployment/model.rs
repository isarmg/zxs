use super::{Input, SmbInput};
use crate::Result;
use data_serialization::Serialize;
use std::net::SocketAddr;

use crate::account::Name as Account;
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
