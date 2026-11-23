//! One explicitly planned complete import to an already initialized DATA disk.
use super::{
    Body, Change, Conflict, DataRoot, DataVolume, Error, Id, MAIN, Mutation, NodeId, Receipt,
    Routes, Store, Target, invalid, qualify_roster, recycle::Recycle, unenrolled_data,
};
use crate::{display, fs::HeldDir, hash::Sha256};
use std::{
    io::{self, Read},
    os::unix::ffi::OsStrExt,
};

const INTENT: &[u8] = b"ZXS-IMPORT-INTENT-V1\0";
const CONTAINER: &[u8] = b"ZXS-IMPORT-CONTAINER-V1\0";
const NODE: &[u8] = b"ZXS-IMPORT-NODE-V1\0";
const PATH_BYTES: usize = 4095;

pub(crate) struct SourceWitness {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) uid: u32,
    pub(crate) mode: u32,
    pub(crate) links: u64,
    pub(crate) modified: (i64, i64),
    pub(crate) changed: (i64, i64),
}

/// Operator-retained immutable intent, with no credential or live mount ID.
/// Caller still proves the actual named source and selected live mount.
pub(crate) struct ImportIntent {
    pub(crate) seed: Id,
    pub(crate) pool: Id,
    pub(crate) data_token: [u8; 32],
    pub(crate) data_path: Vec<u8>,
    pub(crate) source_root: Vec<u8>,
    pub(crate) source_relative: Vec<u8>,
    pub(crate) source: SourceWitness,
    pub(crate) name: String,
    pub(crate) body: Body,
}

impl ImportIntent {
    pub(crate) fn identity(&self) -> io::Result<(Mutation, NodeId)> {
        let intent = self.digest()?;
        let container = derive(CONTAINER, self.seed, intent)?;
        let node = derive(NODE, self.seed, intent)?;
        if container == [0; 16] || node == [0; 16] || container == node {
            return Err(invalid("import intent derived invalid identities"));
        }
        Ok((
            Mutation {
                container,
                request: self.seed,
            },
            node,
        ))
    }

    fn digest(&self) -> io::Result<[u8; 32]> {
        let source = &self.source;
        if self.seed == [0; 16]
            || self.pool == [0; 16]
            || self.data_token == [0; 32]
            || !proper_path(&self.data_path, true)
            || !proper_path(&self.source_root, true)
            || !proper_path(&self.source_relative, false)
            || source.inode == 0
            || source.mode & 0o170_000 != 0o100_000
            || source.links == 0
            || !(0..1_000_000_000).contains(&source.modified.1)
            || !(0..1_000_000_000).contains(&source.changed.1)
        {
            return Err(invalid("invalid immutable import intent"));
        }
        display::component(self.name.as_bytes(), display::MAX)?;
        let mut hash = Sha256::new();
        hash.update(INTENT)?;
        hash.update(&self.pool)?;
        hash.update(&self.data_token)?;
        field(&mut hash, &self.data_path)?;
        field(&mut hash, &self.source_root)?;
        field(&mut hash, &self.source_relative)?;
        hash.update(&source.device.to_be_bytes())?;
        hash.update(&source.inode.to_be_bytes())?;
        hash.update(&source.uid.to_be_bytes())?;
        hash.update(&source.mode.to_be_bytes())?;
        hash.update(&source.links.to_be_bytes())?;
        for (seconds, nanos) in [source.modified, source.changed] {
            hash.update(&seconds.to_be_bytes())?;
            hash.update(&nanos.to_be_bytes())?;
        }
        hash.update(&self.body.length.to_be_bytes())?;
        hash.update(&self.body.sha256)?;
        field(&mut hash, self.name.as_bytes())?;
        Ok(hash.finish())
    }

    fn change(&self, node: NodeId) -> Change {
        Change::Save {
            target: Target::Branch {
                name: MAIN.into(),
                expected_head: None,
            },
            node,
            body: self.body.clone(),
            initial_name: Some(self.name.clone()),
        }
    }
}

fn proper_path(bytes: &[u8], absolute: bool) -> bool {
    if bytes.is_empty() || bytes.len() > PATH_BYTES || bytes.contains(&0) {
        return false;
    }
    let components = if absolute {
        let Some(relative) = bytes.strip_prefix(b"/") else {
            return false;
        };
        relative
    } else {
        bytes
    };
    components
        .split(|&byte| byte == b'/')
        .all(|part| !part.is_empty() && !matches!(part, b"." | b".."))
}

fn field(hash: &mut Sha256, value: &[u8]) -> io::Result<()> {
    let len = u16::try_from(value.len()).map_err(|_| invalid("oversize import intent field"))?;
    hash.update(&len.to_be_bytes())?;
    hash.update(value)
}

fn derive(domain: &[u8], seed: Id, intent: [u8; 32]) -> io::Result<Id> {
    let mut hash = Sha256::new();
    hash.update(domain)?;
    hash.update(&seed)?;
    hash.update(&intent)?;
    let digest = hash.finish();
    let mut first = [0; 16];
    first.copy_from_slice(&digest[..16]);
    Ok(first)
}

/// Keep the caller's original offline EX through this object's lifetime.
/// Only the private import worker may construct/use this writer.
pub(crate) struct OfflineImport {
    store: Store,
    pool: Id,
}

impl OfflineImport {
    pub(crate) fn open(
        system: HeldDir,
        recycle: HeldDir,
        data: Vec<DataRoot>,
        pool: Id,
        require_original: impl Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        require_original()?;
        Recycle::require_roles(&system, &recycle, &data)?;
        let writer = Self::open_existing(system, recycle, data, pool)?;
        require_original()?;
        Ok(writer)
    }

    fn open_existing(
        system: HeldDir,
        recycle: HeldDir,
        mut data: Vec<DataRoot>,
        pool: Id,
    ) -> io::Result<Self> {
        qualify_roster(&mut data)?;
        let owner = system.existing_opaque_owner(c"container-owner")?;
        let routes = Routes::open_existing(&system)?
            .ok_or_else(|| invalid("the already initialized SYSTEM routes are absent"))?;
        let volumes = data
            .into_iter()
            .map(DataVolume::open_existing)
            .collect::<io::Result<Vec<_>>>()?;
        let archive = Recycle::open_existing(system, recycle, pool)?;
        let mut store = Store::from_roster(volumes, routes, owner);
        store.archive = Some(archive);
        Ok(Self { store, pool })
    }

    fn place(&self, intent: &ImportIntent, id: Id) -> Result<(&DataVolume, bool), Error> {
        if intent.pool != self.pool {
            return Err(invalid("import intent belongs to another pool").into());
        }
        let target = self
            .store
            .enrolled(intent.data_token)
            .ok_or_else(unenrolled_data)?;
        target.require_named()?;
        if target.root().path.as_os_str().as_bytes() != intent.data_path.as_slice() {
            return Err(invalid("import target is not the originally chosen enrolled DATA").into());
        }
        let route = self.store.routes.get(id)?;
        if route.is_some_and(|token| token != intent.data_token) {
            return Err(invalid("import identity was routed to a different DATA").into());
        }
        Ok((target, route.is_some()))
    }

    fn selected(
        &self,
        mutation: Mutation,
        change: &Change,
        routed: bool,
    ) -> Result<Option<Receipt>, Error> {
        let Some(receipt) = self.store.receipt(mutation)? else {
            return Ok(None);
        };
        if !routed
            || receipt.mutation != mutation
            || receipt.sequence != 1
            || receipt.change != *change
        {
            return Err(Conflict::RequestReused.into());
        }
        Ok(Some(receipt))
    }

    /// Confirm a previous exact first receipt without asking for a source body.
    pub(crate) fn receipt(
        &self,
        intent: &ImportIntent,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Option<Receipt>, Error> {
        let (mutation, node) = intent.identity()?;
        authorize()?;
        let (_, routed) = self.place(intent, mutation.container)?;
        let found = self.selected(mutation, &intent.change(node), routed)?;
        authorize()?;
        Ok(found)
    }

    /// Apply only a complete first-main node to this intent's exact DATA.
    pub(crate) fn save(
        &self,
        intent: &ImportIntent,
        input: impl Read,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        let (mutation, node) = intent.identity()?;
        let change = intent.change(node);
        authorize()?;
        let (target, routed) = self.place(intent, mutation.container)?;
        if let Some(receipt) = self.selected(mutation, &change, routed)? {
            authorize()?;
            return Ok(receipt);
        }
        if self.store.recycled(mutation.container)?.is_some() {
            return Err(Conflict::MissingContainer.into());
        }
        if routed
            && self
                .store
                .project_container(mutation.container, true, |_, _| ())?
                .is_some()
        {
            return Err(Conflict::AlreadyExists.into());
        }
        authorize()?;
        self.store
            .save_on(mutation, change, input, target, authorize)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/import_one.rs"]
mod tests;
