use super::model::Settings;
use data_serialization::Serialize;
use sha2::{Digest, Sha256};

#[derive(Serialize)]
pub(super) struct ConfigurationFile {
    name: &'static str,
    sha256: String,
    pub content: String,
}
impl ConfigurationFile {
    fn new(name: &'static str, content: String) -> Self {
        let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
        Self {
            name,
            sha256,
            content,
        }
    }
}
pub(super) fn files(settings: &Settings) -> (ConfigurationFile, Option<ConfigurationFile>) {
    let smb_args = match &settings.smb {
        Some(smb) => format!(
            " --smb-socket /zxs/run/smb.sock --smb-peer-uid {} --smb-user {}",
            smb.peer_uid,
            smb.account.as_str()
        ),
        None => String::new(),
    };
    let unit = include_str!("../../deploy/zxs.service.in")
        .replace("@USER@", settings.user.as_str())
        .replace("@GROUP@", settings.group.as_str())
        .replace("@LISTEN@", &settings.listen.to_string())
        .replace("@SMB_ARGS@", &smb_args);
    let samba = settings.smb.as_ref().map(|smb| {
        ConfigurationFile::new(
            "smb.conf",
            include_str!("../../deploy/smb.conf.in").replace("@SMB_ACCOUNT@", smb.account.as_str()),
        )
    });
    (ConfigurationFile::new("zxs.service", unit), samba)
}
