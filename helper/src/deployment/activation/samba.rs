use crate::{
    Result,
    account::LocalAccount,
    installation::vfs_payload::{self, HOST_VERSION, MODULE_DIRECTORY, MODULE_LIMIT},
    private_io::FixedProgramParents,
    system_program::{Executable, FileStamp, Program},
};
use json_codec::{Value, json};
use platform_io::fs_safety::{EntryName, RelativePath, linux::SingleLinkRequirement};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    os::unix::fs::{FileExt, MetadataExt},
};

pub(super) struct Samba {
    daemon: Executable,
    testparm: Executable,
    pdbedit: Executable,
    module_root: FixedProgramParents,
    module: File,
    module_stamp: FileStamp,
    module_hash: String,
}
impl Samba {
    pub fn open(account: &LocalAccount, peer_uid: u32) -> Result<Self> {
        // The dedicated smbd runs its trusted bridge connection as root. Do
        // not silently start it against an unrelated configured peer identity.
        if peer_uid != 0 {
            return Err("the supported dedicated Samba unit requires the explicitly configured trusted peer UID 0".into());
        }
        let daemon = module_host()?;
        let expected = vfs_payload::required()?;
        let testparm = Executable::open(Program::Testparm)?;
        let pdbedit = Executable::open(Program::Pdbedit)?;
        super::super::samba_password::require_activation(&testparm, &pdbedit, &account.snapshot())?;
        let module_root = FixedProgramParents::vfs()?;
        let module = module_root
            .leaf()
            .open_file(&RelativePath::new("zxs.so")?, false)?;
        let (module_stamp, module_hash) = module_identity(&module)?;
        if module_hash != expected.sha256() {
            return Err("installed VFS differs from the helper's sealed same-source payload; no different module was loaded or replaced".into());
        }
        let result = Self {
            daemon,
            testparm,
            pdbedit,
            module_root,
            module,
            module_stamp,
            module_hash,
        };
        result.recheck()?;
        Ok(result)
    }
    pub fn snapshot(&self) -> Value {
        json!({"daemon":self.daemon.snapshot(), "testparm":self.testparm.snapshot(),
            "pdbedit":self.pdbedit.snapshot(), "vfs_path":format!("{MODULE_DIRECTORY}/zxs.so"),
            "vfs":self.module_stamp, "vfs_sha256":self.module_hash,
            "required_abi":HOST_VERSION, "network_password_login_verified":false})
    }
    pub fn recheck(&self) -> Result<()> {
        self.daemon.recheck()?;
        self.testparm.recheck()?;
        self.pdbedit.recheck()?;
        self.module_root.require_current()?;
        let root = self.module_root.leaf();
        let name = EntryName::new("zxs.so")?;
        let original = SingleLinkRequirement::check(&self.module)?;
        root.verify_entry_identity(&name, original)?;
        if module_identity(&self.module)? != (self.module_stamp.clone(), self.module_hash.clone()) {
            return Err(
                "installed Samba module changed; do not activate from a substituted source".into(),
            );
        }
        // A legitimate module read can be long. The path itself must still lead
        // through every original ancestor to the same leaf after hashing.
        self.module_root.require_current()?;
        root.verify_entry_identity(&name, original)?;
        Ok(())
    }
}
pub(super) fn module_host() -> Result<Executable> {
    let daemon = Executable::open(Program::Smbd)?;
    let version = daemon.capture(&["--version"])?;
    if std::str::from_utf8(&version)?.trim() != HOST_VERSION {
        return Err("install the exact Debian 13 amd64 Samba 4.22.11+dfsg-0+deb13u1 build required by this ZXS VFS; no packages were installed".into());
    }
    let build = daemon.capture(&["--build-options"])?;
    let build = std::str::from_utf8(&build)?;
    let modules = build
        .lines()
        .filter_map(|line| line.trim().strip_prefix("MODULESDIR:"))
        .map(str::trim)
        .collect::<Vec<_>>();
    if modules.as_slice() != ["/usr/lib/x86_64-linux-gnu/samba"] {
        return Err("Samba module directory differs from the supported target; no library search path was changed".into());
    }
    Ok(daemon)
}

fn module_identity(file: &File) -> Result<(FileStamp, String)> {
    SingleLinkRequirement::check(file)?;
    let before = file.metadata()?;
    if before.uid() != 0
        || before.mode() & 0o6022 != 0
        || !(64..=MODULE_LIMIT).contains(&before.len())
    {
        return Err("install the root-owned non-setid ZXS VFS ELF from the matching release; no module or permissions were changed".into());
    }
    let mut header = [0_u8; 5];
    file.read_exact_at(&mut header, 0)?;
    if header != *b"\x7fELF\x02" {
        return Err("the installed ZXS VFS is not a 64-bit ELF library".into());
    }
    let mut hash = Sha256::new();
    let mut block = [0_u8; 32 * 1024];
    let mut offset = 0;
    loop {
        let count = file.read_at(&mut block, offset)?;
        if count == 0 {
            break;
        }
        offset += count as u64;
        if offset > MODULE_LIMIT {
            return Err("installed VFS exceeded its byte bound".into());
        }
        hash.update(&block[..count]);
    }
    let stamp = FileStamp::of(&before);
    if offset != before.len() || FileStamp::of(&file.metadata()?) != stamp {
        return Err("installed VFS changed during its native read".into());
    }
    Ok((stamp, platform_io::lower_hex(&hash.finalize())))
}
