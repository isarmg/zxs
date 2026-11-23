//! Offline retirement of a clean DATA root. The disk itself is never changed.
use super::add_data::{finish_stage, read_stage};
use super::*;
use crate::headless_setup::{Held, MARKER, read_role_token};
use rustix::fs::{RenameFlags, mkdirat, renameat_with};

pub(super) const RETIRE_STAGE: &str = ".server-stage-retire-data";
const RETIRED: &str = "retired-data";
const DATA_ROLE: u8 = 1;
const ROLE_ONLY: u8 = 0b0001;
const INITIALIZED: u8 = 0b1111;

impl FreshSelection {
    pub(crate) fn retire_data(self, target: &Path) -> io::Result<()> {
        let paths: Vec<_> = self.paths().map(Path::to_path_buf).collect();
        let index = paths
            .iter()
            .position(|path| path == target)
            .ok_or_else(refused)?;
        if index < FIXED_ROOTS || paths.len() <= FIXED_ROOTS + 1 {
            return Err(refused());
        }
        match self.open_offline(Some(RETIRE_STAGE)) {
            Ok(reset) => reset.retire(index),
            Err(error) => {
                let mut remaining = paths;
                remaining.remove(index);
                let reset = Self::select(&remaining)
                    .and_then(FreshSelection::begin_reset)
                    .map_err(|_| error)?;
                let selected = &reset.selected;
                let original = Held::select(target, selected.uid, &selected.scope)?;
                let (_, token) =
                    read_role_token(&original.input(), DATA_ROLE, &selected.pool, selected.uid)?;
                if selected.tokens.contains(&token) {
                    return Err(refused());
                }
                let system = &selected.roots[0].directory;
                if !retired(system, &selected.pool, &token, selected.uid)? {
                    return Err(refused());
                }
                reset.require_original()
            }
        }
    }
}

impl Reset {
    fn retire(&self, index: usize) -> io::Result<()> {
        let selected = &self.selected;
        let uid = selected.uid;
        let system = &selected.roots[0].directory;
        let target = &selected.roots[index].directory;
        let token = selected.tokens[index];
        let pool = &selected.pool;
        let prove = || {
            self.require_original()?;
            let initialized = match data_members(target)? {
                ROLE_ONLY => false,
                INITIALIZED => true,
                _ => return Err(refused()),
            };
            archive_routes(system, &token, uid, initialized)?;
            prove_target(target, uid, initialized)?;
            self.require_original()
        };
        prove()?;

        let count_at = LOGIN + 1 + self.login.len();
        let mut updated = Zeroizing::new(self.original.full.to_vec());
        let mut tail = &updated[count_at + 1..];
        let mut first = count_at + 1;
        for _ in 0..index {
            first += skip_root(&mut tail)?;
        }
        let end = first + skip_root(&mut tail)?;
        updated.drain(first..end);
        updated[count_at] -= 1;
        if let Some(stage) = read_stage(&self.locked, RETIRE_STAGE, uid)? {
            if !updated.starts_with(&stage.full) || !retired(system, pool, &token, uid)? {
                return Err(refused());
            }
        }

        if child(system, RETIRED, uid)?.is_none() {
            prove()?;
            mkdirat(system, RETIRED, Mode::RUSR | Mode::WUSR | Mode::XUSR)?;
        }
        let tombstones = child(system, RETIRED, uid)?.ok_or_else(refused)?;
        let name = hex(&token);
        let proof = proof_bytes(pool, &token);
        let prove_directory = || {
            prove()?;
            require_named(system, RETIRED, &tombstones, uid)
        };
        if let Some(record) = read_stage(&tombstones, &name, uid)? {
            if record.full.as_slice() != proof {
                return Err(refused());
            }
            record.held.sync_all()?;
        } else {
            let stage = format!(".{name}.incoming");
            let record = finish_stage(&tombstones, &stage, &proof, uid, prove_directory)?;
            prove_directory()?;
            if stamp(&read(&tombstones, &stage, uid)?.meta) != stamp(&record.meta) {
                return Err(refused());
            }
            renameat_with(
                &tombstones,
                stage.as_str(),
                &tombstones,
                name.as_str(),
                RenameFlags::NOREPLACE,
            )?;
        }
        tombstones.sync_all()?;
        system.sync_all()?;
        let ready = || {
            prove_directory()?;
            if !retired(system, pool, &token, uid)? {
                return Err(refused());
            }
            Ok(())
        };
        let stage = finish_stage(&self.locked, RETIRE_STAGE, &updated, uid, ready)?;
        ready()?;
        let named = read(&self.locked, RETIRE_STAGE, uid)?;
        if stamp(&named.meta) != stamp(&stage.meta) || named.full.as_slice() != updated.as_slice() {
            return Err(refused());
        }
        renameat(&self.locked, RETIRE_STAGE, &self.locked, DESCRIPTOR)?;
        self.locked.sync_all()?;
        let final_record = read(&self.locked, DESCRIPTOR, uid)?;
        if Entity::of(&final_record.meta)? != Entity::of(&stage.meta)?
            || final_record.full.as_slice() != updated.as_slice()
        {
            return Err(refused());
        }
        Ok(())
    }
}

fn data_members(dir: &File) -> io::Result<u8> {
    // A fresh reader of the original directory, without updating atime.
    let reader = File::from(openat2(
        dir,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOATIME,
        Mode::empty(),
        SCOPED,
    )?);
    let mut buffer = [MaybeUninit::uninit(); 4096];
    let mut rows = RawDir::new(&reader, &mut buffer);
    let mut seen = 0;
    while let Some(row) = rows.next() {
        let row = row?;
        let bit = match row.file_name().to_bytes() {
            b"." | b".." => continue,
            mark if mark == MARKER.as_bytes() => ROLE_ONLY,
            b"owner" => 0b0010,
            b"data" => 0b0100,
            b"staging" => 0b1000,
            _ => return Err(refused()),
        };
        if seen & bit != 0 {
            return Err(refused());
        }
        seen |= bit;
    }
    Ok(seen)
}

fn open_owner(dir: &File, uid: u32) -> io::Result<File> {
    // Core's DATA owner lease is a private empty regular inode, not a payload.
    // Open it for metadata only, so no unfamiliar node is opened for data I/O.
    let file = File::from(openat2(
        dir,
        "owner",
        OFlags::PATH | OFlags::CLOEXEC,
        Mode::empty(),
        SCOPED,
    )?);
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.ino() == 0
        || meta.uid() != uid
        || meta.nlink() != 1
        || meta.mode() & 0o7777 != 0o600
        || meta.len() != 0
    {
        return Err(refused());
    }
    Ok(file)
}

fn prove_target(target: &File, uid: u32, initialized: bool) -> io::Result<()> {
    if initialized {
        let owner = open_owner(target, uid)?;
        let expected = stamp(&owner.metadata()?);
        let data = child(target, "data", uid)?.ok_or_else(refused)?;
        let staging = child(target, "staging", uid)?.ok_or_else(refused)?;
        let dirs = [("data", &data), ("staging", &staging)];
        for (_, dir) in dirs {
            if data_members(dir)? != 0 {
                return Err(refused());
            }
            dir.sync_all()?;
        }
        target.sync_all()?;
        for (name, dir) in dirs {
            if data_members(dir)? != 0 {
                return Err(refused());
            }
            require_named(target, name, dir, uid)?;
        }
        if stamp(&owner.metadata()?) != expected
            || stamp(&open_owner(target, uid)?.metadata()?) != expected
        {
            return Err(refused());
        }
    } else {
        target.sync_all()?;
    }
    let expected = if initialized { INITIALIZED } else { ROLE_ONLY };
    if data_members(target)? != expected {
        return Err(refused());
    }
    Ok(())
}

fn skip_root(bytes: &mut &[u8]) -> io::Result<usize> {
    let width = usize::from(u16::from_be_bytes(fixed(bytes)?));
    field(bytes, width + 32)?;
    Ok(2 + width + 32)
}

fn child(parent: &File, name: &str, uid: u32) -> io::Result<Option<File>> {
    match openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        SCOPED,
    ) {
        Ok(fd) => {
            let file = File::from(fd);
            let info = file.metadata()?;
            if info.uid() != uid || info.mode() & 0o7777 != 0o700 {
                return Err(refused());
            }
            Ok(Some(file))
        }
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn require_named(parent: &File, name: &str, original: &File, uid: u32) -> io::Result<()> {
    let named = child(parent, name, uid)?.ok_or_else(refused)?;
    if Entity::of(&original.metadata()?)? != Entity::of(&named.metadata()?)? {
        return Err(refused());
    }
    Ok(())
}

fn proof_bytes(pool: &[u8; 16], token: &[u8; 32]) -> [u8; 52] {
    let mut proof = [0; 52];
    proof[..4].copy_from_slice(b"DT1\0");
    proof[4..20].copy_from_slice(pool);
    proof[20..].copy_from_slice(token);
    proof
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                DIGITS[usize::from(byte >> 4)],
                DIGITS[usize::from(byte & 15)],
            ]
        })
        .map(char::from)
        .collect()
}

pub(super) fn retired(
    system: &File,
    pool: &[u8; 16],
    token: &[u8; 32],
    uid: u32,
) -> io::Result<bool> {
    let Some(dir) = child(system, RETIRED, uid)? else {
        return Ok(false);
    };
    let Some(record) = read_stage(&dir, &hex(token), uid)? else {
        return Ok(false);
    };
    if record.full.as_slice() != proof_bytes(pool, token) {
        return Err(refused());
    }
    require_named(system, RETIRED, &dir, uid)?;
    Ok(true)
}

fn archive_routes(system: &File, token: &[u8; 32], uid: u32, initialized: bool) -> io::Result<()> {
    let Some(routes) = child(system, "container-routes", uid)? else {
        return Ok(());
    };
    let moves = child(system, "container-moves", uid)?;
    let mut buffer = [MaybeUninit::uninit(); 4096];
    let mut entries = RawDir::new(&routes, &mut buffer);
    while let Some(entry) = entries.next() {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        let stage = bytes
            .strip_prefix(b".")
            .and_then(|x| x.strip_suffix(b".incoming"));
        let key = stage.unwrap_or(bytes);
        if key.len() != 32
            || !key
                .iter()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        {
            return Err(refused());
        }
        if stage.is_some() {
            continue;
        }
        let name = std::str::from_utf8(key).map_err(|_| refused())?;
        let record = read(&routes, name, uid)?;
        let raw = record.full.as_slice();
        if raw.len() != 52
            || &raw[..4] != b"CR1\0"
            || hex(&raw[4..20]) != name
            || raw[4..20] == [0; 16]
            || raw[20..] == [0; 32]
        {
            return Err(refused());
        }
        if &raw[20..] != token {
            continue;
        }
        if !initialized {
            return Err(io::Error::other(
                "a marker-only DATA disk has a selected container route",
            ));
        }
        let area = moves.as_ref().ok_or_else(refused)?;
        let selected = read(area, name, uid)?;
        let receipt = selected.full.as_slice();
        if receipt.len() != 108
            || &receipt[..4] != b"RC1\0"
            || receipt[4..20] != raw[4..20]
            || receipt[20..36] == [0; 16]
            || receipt[36..44] == [0; 8]
            || receipt[44..52] == receipt[60..68]
        {
            return Err(io::Error::other(
                "an enrolled route has no valid selected recycle record",
            ));
        }
        selected.held.sync_all()?;
    }
    if let Some(moves) = moves {
        moves.sync_all()?;
        require_named(system, "container-moves", &moves, uid)?;
    }
    routes.sync_all()?;
    require_named(system, "container-routes", &routes, uid)?;
    system.sync_all()
}
