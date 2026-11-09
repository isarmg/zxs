//! A missing-account proof for the exact native local passdb. This does not
//! create a database; only the separately confirmed smbpasswd command may do so.
use super::*;
use platform_io::fs_safety::Error as FileError;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::ErrorKind};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::deployment::samba_password) struct Baseline {
    parent_device: String,
    parent_inode: String,
    database: Option<FileStamp>,
    accounts_digest: String,
}
#[derive(PartialEq, Eq, Serialize)]
struct Account {
    uid: u32,
    description: String,
}
type Accounts = BTreeMap<String, Account>;
struct Existing {
    file: File,
    identity: FileIdentity,
}
pub(in crate::deployment::samba_password) struct Target {
    parents: Vec<Parent>,
    existing: Option<Existing>,
    accounts: Accounts,
    baseline: Baseline,
    _lock: AdvisoryLock,
}
impl Target {
    pub(in crate::deployment::samba_password) fn open(
        program: &Executable,
        account: &Snapshot,
    ) -> Result<Self> {
        let parents = parents()?;
        let private = parents
            .last()
            .expect("invariant: fixed Samba private parent");
        let lock = AdvisoryLock::directory(private.root.directory())?;
        let file = optional_database(private)?;
        let existing = file
            .map(|file| -> Result<Existing> {
                Ok(Existing {
                    identity: check_database(&file)?,
                    file,
                })
            })
            .transpose()?;
        let database = existing
            .as_ref()
            .map(|value| value.file.metadata().map(|metadata| FileStamp::of(&metadata)))
            .transpose()?;
        // A list command must never initialize a missing database during preview.
        let accounts = if existing.is_some() {
            accounts(program)?
        } else {
            Accounts::new()
        };
        if accounts.iter().any(|(name, existing)| {
            name.eq_ignore_ascii_case(account.name.as_str()) || existing.uid == account.uid
        }) {
            return Err("the selected Samba name or Unix UID already has a native account; use the separate existing-account reset instead".into());
        }
        let baseline = Baseline {
            parent_device: private.root.identity().device.to_string(),
            parent_inode: private.root.identity().inode.to_string(),
            database,
            accounts_digest: platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(
                &accounts,
            )?)),
        };
        let result = Self {
            parents,
            existing,
            accounts,
            baseline,
            _lock: lock,
        };
        result.require_original()?;
        Ok(result)
    }
    fn private(&self) -> &Parent {
        self.parents
            .last()
            .expect("invariant: fixed Samba private parent")
    }
    fn recheck_parents(&self) -> Result<()> {
        for parent in &self.parents {
            parent.recheck()?;
        }
        Ok(())
    }
    pub(in crate::deployment::samba_password) fn baseline(&self) -> Baseline {
        self.baseline.clone()
    }
    pub(in crate::deployment::samba_password) fn creates_database(&self) -> bool {
        self.existing.is_none()
    }
    pub(in crate::deployment::samba_password) fn require_original(&self) -> Result<()> {
        self.recheck_parents()?;
        match (&self.existing, optional_database(self.private())?) {
            (None, None) => Ok(()),
            (Some(original), Some(named)) => {
                if check_database(&named)? != original.identity
                    || check_database(&original.file)? != original.identity
                    || Some(FileStamp::of(&original.file.metadata()?)) != self.baseline.database
                {
                    return Err("native Samba database changed after missing-account review; enrollment was not admitted".into());
                }
                Ok(())
            }
            _ => Err(
                "native Samba database presence changed after review; preserve the original request"
                    .into(),
            ),
        }
    }
    pub(in crate::deployment::samba_password) fn complete(
        &self,
        program: &Executable,
        account: &Snapshot,
    ) -> Result<Revision> {
        self.recheck_parents()?;
        let file = optional_database(self.private())?
            .ok_or("Samba enrollment did not leave its native database; outcome is uncertain")?;
        let identity = check_database(&file)?;
        if let Some(original) = &self.existing
            && (identity != original.identity || check_database(&original.file)? != identity)
        {
            return Err("Samba enrollment replaced the admitted database inode; preserve all material".into());
        }
        let mut current = accounts(program)?;
        let selected = current
            .remove(account.name.as_str())
            .ok_or("Samba enrollment did not return the exact selected account")?;
        if selected.uid != account.uid || current != self.accounts {
            return Err("Samba enrollment account readback differs or changed another account; preserve the uncertain operation".into());
        }
        file.sync_all()?;
        self.private().root.directory().sync_all()?;
        let after = Revision {
            parent_device: self.baseline.parent_device.clone(),
            parent_inode: self.baseline.parent_inode.clone(),
            file: FileStamp::of(&file.metadata()?),
        };
        self.require_after(&after)?;
        Ok(after)
    }
    pub(in crate::deployment::samba_password) fn require_after(
        &self,
        expected: &Revision,
    ) -> Result<()> {
        self.recheck_parents()?;
        let file = optional_database(self.private())?
            .ok_or("completed Samba enrollment database is missing; it was not recreated")?;
        check_database(&file)?;
        if expected.parent_device != self.baseline.parent_device
            || expected.parent_inode != self.baseline.parent_inode
            || FileStamp::of(&file.metadata()?) != expected.file
        {
            return Err("Samba enrollment database changed after readback; the receipt is historical only".into());
        }
        Ok(())
    }
}
fn optional_database(parent: &Parent) -> Result<Option<File>> {
    match parent
        .root
        .open_file(&RelativePath::new("passdb.tdb")?, false)
    {
        Ok(file) => Ok(Some(file)),
        Err(FileError::Io(error)) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn accounts(program: &Executable) -> Result<Accounts> {
    let bytes = program.capture(&[
        "--debuglevel=0",
        "--configfile",
        crate::private_io::SAMBA_PATH,
        "--list",
    ])?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "native Samba account listing is not UTF-8")?;
    let mut result = Accounts::new();
    for line in text.lines() {
        if line.is_empty() || line.chars().any(char::is_control) {
            return Err("native Samba account listing contains an invalid record".into());
        }
        let mut fields = line.splitn(3, ':');
        let name = fields.next().ok_or("native Samba account record has no name")?;
        let uid = fields
            .next()
            .ok_or("native Samba account record has no UID")?
            .parse::<u32>()
            .map_err(|_| "native Samba account UID is malformed")?;
        let description = fields
            .next()
            .ok_or("native Samba account record is incomplete")?;
        if name.is_empty()
            || result
                .insert(
                    name.into(),
                    Account {
                        uid,
                        description: description.into(),
                    },
                )
                .is_some()
        {
            return Err("native Samba account listing has an empty or repeated name".into());
        }
    }
    Ok(result)
}
