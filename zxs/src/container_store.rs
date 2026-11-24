//! Complete opaque node saves and metadata selection for individual containers.
mod codec;
pub(crate) mod import_one;
pub(crate) mod init_data;
mod model;
mod name_index;
pub(crate) mod offline_catalog;
mod operation;
mod payload;
mod placement;
mod recycle;
mod recycle_material;
mod recycle_receipt;
mod routes;
mod stage;
#[cfg(test)]
pub(crate) mod upload;
mod volume;

#[cfg(test)]
use crate::fs::local_uid;
use crate::fs::{HeldDir, ReadAccess};
#[cfg(test)]
use crate::hash::Sha256;
pub(crate) use model::{Body, Change, Conflict, Id, MAIN, Mutation, Node, NodeId, Receipt, Target};
use model::{Decision, State};
use name_index::{Background, ChangeCut, Clock, Names, Ticket};
pub(crate) use operation::OperationReceipt;
use placement::{Intake, Placement, Sample};
pub(crate) use recycle::RecycleReceipt;
use routes::Routes;
#[cfg(target_arch = "x86_64")]
pub(crate) use routes::data_move_codec::{
    Completion as MoveCompletion, MoveRequest, MoveRoles, execute_move, require_move_absent,
    require_move_scope, require_stage_absent,
};
use stage::{FlatStage, NameCut, Publication};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fmt,
    fs::File,
    io::{self, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use volume::DataVolume;

#[cfg(test)]
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const TAG_BYTES: usize = 1024 * 1024;
const OBJECT_SLOT_TARGET: usize = 128;
const PLACEMENT_ATTEMPTS: usize = 4;

pub(crate) enum Error {
    Io(io::Error),
    Conflict(Conflict),
    WrongLength,
    WrongDigest,
    Uncertain(io::Error),
    CleanupPending {
        receipt: Box<RecycleReceipt>,
        error: io::Error,
    },
}

// A local diagnostic can name the disposition and error kind without
// rendering arbitrary I/O details or the selected receipt.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => f.debug_tuple("Io").field(&error.kind()).finish(),
            Self::Conflict(reason) => f.debug_tuple("Conflict").field(reason).finish(),
            Self::WrongLength => f.write_str("WrongLength"),
            Self::WrongDigest => f.write_str("WrongDigest"),
            Self::Uncertain(error) => f.debug_tuple("Uncertain").field(&error.kind()).finish(),
            Self::CleanupPending { error, .. } => f
                .debug_struct("CleanupPending")
                .field("kind", &error.kind())
                .finish(),
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<Conflict> for Error {
    fn from(value: Conflict) -> Self {
        Self::Conflict(value)
    }
}

pub(crate) const CATALOG_PAGE_ROWS: usize = 256;

pub(crate) struct CatalogRow {
    pub(crate) id: Id,
    pub(crate) sequence: u64,
    pub(crate) name: String,
}

pub(crate) struct CatalogPage {
    pub(crate) items: Vec<CatalogRow>,
    pub(crate) next_after: Option<Id>,
}

fn catalog_rows(
    sorted: impl IntoIterator<Item = Id>,
    after: Option<Id>,
    readable: impl Fn(Id) -> bool,
    mut project: impl FnMut(Id) -> io::Result<Option<CatalogRow>>,
) -> io::Result<CatalogPage> {
    let mut items = Vec::with_capacity(CATALOG_PAGE_ROWS);
    for id in sorted {
        if after.is_some_and(|bound| id <= bound) || !readable(id) {
            continue;
        }
        if let Some(row) = project(id)? {
            items.push(row);
            if items.len() == CATALOG_PAGE_ROWS {
                return Ok(CatalogPage {
                    items,
                    next_after: Some(id),
                });
            }
        }
    }
    Ok(CatalogPage {
        items,
        next_after: None,
    })
}

pub(crate) const HISTORY_PAGE_ROWS: usize = 256;

pub(crate) struct HistoryPage {
    pub(crate) head: NodeId,
    pub(crate) nodes: Vec<(NodeId, Node)>,
    pub(crate) next: Option<NodeId>,
}

fn select_history_page(
    state: &State,
    branch: &str,
    head: NodeId,
    next: Option<NodeId>,
) -> Result<HistoryPage, Error> {
    let current = *state.branches.get(branch).ok_or(Conflict::MissingBranch)?;
    if current != head {
        return Err(Conflict::HeadChanged(current).into());
    }
    if next == Some(head) {
        return Err(Conflict::MissingNode.into());
    }
    let first = next.unwrap_or(head);
    let mut at = Some(head);
    let mut previous = None;
    let mut emitted = false;
    let mut nodes = Vec::with_capacity(HISTORY_PAGE_ROWS);
    while let Some(id) = at {
        let node = state
            .nodes
            .get(&id)
            .ok_or_else(|| invalid("branch refers to a missing node"))?;
        if node.sequence == 0
            || node.sequence > state.sequence
            || previous.is_some_and(|upper| node.sequence >= upper)
        {
            return Err(invalid("branch ancestry is not strictly descending").into());
        }
        emitted |= id == first;
        if emitted {
            if nodes.len() == HISTORY_PAGE_ROWS {
                return Ok(HistoryPage {
                    head,
                    nodes,
                    next: Some(id),
                });
            }
            nodes.push((id, node.clone()));
        }
        previous = Some(node.sequence);
        at = node.prior;
    }
    if !emitted {
        return Err(Conflict::MissingNode.into());
    }
    Ok(HistoryPage {
        head,
        nodes,
        next: None,
    })
}

pub(crate) const DETAIL_PAGE_ROWS: usize = 256;

pub(crate) struct DetailPage {
    pub(crate) id: Id,
    pub(crate) sequence: u64,
    pub(crate) name: String,
    pub(crate) tags: Vec<String>,
    pub(crate) heads: Vec<(String, NodeId)>,
    pub(crate) next_after: Option<String>,
}

fn select_detail_page(
    id: Id,
    name: &str,
    state: &State,
    sequence: Option<u64>,
    after: Option<&str>,
) -> Result<DetailPage, Error> {
    use std::ops::Bound::{Excluded, Unbounded};

    if sequence.is_some_and(|sequence| sequence != state.sequence) {
        return Err(Conflict::SequenceChanged(state.sequence).into());
    }
    let mut heads = Vec::with_capacity(DETAIL_PAGE_ROWS);
    for (branch, node) in state
        .branches
        .range::<str, _>((after.map_or(Unbounded, Excluded), Unbounded))
        .take(DETAIL_PAGE_ROWS)
    {
        heads.push((branch.clone(), *node));
    }
    let next_after = heads
        .last()
        .filter(|_| heads.len() == DETAIL_PAGE_ROWS)
        .map(|(branch, _)| branch.clone());
    Ok(DetailPage {
        id,
        sequence: state.sequence,
        name: name.to_owned(),
        tags: state.tags.clone(),
        heads,
        next_after,
    })
}

pub(crate) struct NodeBytes {
    pub(crate) node: Node,
    pub(crate) file: File,
}

pub(crate) struct DataRoot {
    pub(crate) token: [u8; 32],
    pub(crate) root: HeldDir,
}

pub(crate) struct Store {
    volumes: Vec<DataVolume>,
    routes: Routes,
    next_stage: AtomicU64,
    placement: Placement,
    // Catalogue lock selects an independent short per-container lock.
    // No upload reads, hashing, or fsync while acquiring this map.
    objects: Mutex<BTreeMap<Id, Arc<Mutex<Option<State>>>>>,
    // Only bootstrap and rename require a shared display-namespace cut.
    names: Mutex<Names>,
    name_clock: Clock,
    archive: Option<recycle::Recycle>,
    system_owner: File,
    #[cfg(test)]
    uploads: upload::Shared,
}

impl Store {
    /// Startup supplies the already-qualified original roots.
    pub(crate) fn open_with_recycle(
        system: HeldDir,
        data: Vec<DataRoot>,
        recycled: HeldDir,
        pool: Id,
        recycle_token: [u8; 32],
    ) -> io::Result<Self> {
        let role = recycle_receipt::RecycleRole {
            pool,
            token: recycle_token,
        };
        role.require()?;
        if data.iter().any(|volume| volume.token == recycle_token) {
            return Err(invalid("DATA and recycle cannot share an enrolled token"));
        }
        recycle::Recycle::require_roles(&system, &recycled, &data)?;
        let owner = system.opaque_owner(c"container-owner")?;
        let mut store = Self::open_roster(&system, data, owner)?;
        store.archive = Some(recycle::Recycle::open(system, recycled, role)?);
        Ok(store)
    }

    fn open_roster(system: &HeldDir, mut data: Vec<DataRoot>, owner: File) -> io::Result<Self> {
        qualify_roster(&mut data)?;
        let routes = Routes::open_existing(system)?;
        let fresh = match &routes {
            Some(routes) => match routes.ids()?.first() {
                Some(id) => {
                    routes
                        .get(*id)?
                        .ok_or_else(|| invalid("selected route vanished at startup"))?;
                    false
                }
                None => true,
            },
            None => true,
        };
        if fresh && let Some(old) = system.child_directory(c"container-moves")? {
            old.only(None)?;
        }
        // An absent route layout is a new enrollment, not a legacy import.
        // Check every DATA namespace under its own owner before creating it.
        let volumes = data
            .into_iter()
            .map(|root| DataVolume::open(root, fresh))
            .collect::<io::Result<Vec<_>>>()?;
        let routes = match routes {
            Some(routes) => routes,
            None => Routes::open(system)?,
        };
        Ok(Self::from_roster(volumes, routes, owner))
    }

    fn from_roster(volumes: Vec<DataVolume>, routes: Routes, owner: File) -> Self {
        Self {
            volumes,
            routes,
            next_stage: AtomicU64::new(0),
            placement: Placement::default(),
            objects: Mutex::new(BTreeMap::new()),
            names: Mutex::new(Names::default()),
            name_clock: Clock::default(),
            archive: None,
            system_owner: owner,
            #[cfg(test)]
            uploads: upload::Shared::default(),
        }
    }

    fn name_observed<T>(&self, result: io::Result<T>) -> io::Result<T> {
        if result.is_err() {
            self.name_clock.fault();
        }
        result
    }

    fn name_background(&self) -> io::Result<Background<'_>> {
        self.names
            .lock()
            .map_err(|_| invalid("container name selection unavailable"))?
            .enter_background(&self.name_clock)
    }

    fn enrolled(&self, token: [u8; 32]) -> Option<&DataVolume> {
        self.volumes
            .binary_search_by_key(&token, |volume| volume.token)
            .ok()
            .map(|i| &self.volumes[i])
    }

    fn routed(&self, id: Id) -> io::Result<Option<&DataVolume>> {
        let backed = self.routes.get(id).and_then(|token| {
            token
                .map(|token| self.enrolled(token).ok_or_else(unenrolled_data))
                .transpose()
        });
        self.name_observed(backed)
    }

    fn write_volume(
        &self,
        id: Id,
        first_body: Option<u64>,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> Result<(&DataVolume, Option<Intake<'_>>), Error> {
        if self.recycled(id)?.is_none() {
            if let Some(volume) = self.routed(id)? {
                return Ok((volume, None));
            }
            if let Some(bytes) = first_body {
                return self.first_volume(id, bytes, authorize);
            }
        }
        authorize()?;
        Err(Conflict::MissingContainer.into())
    }

    fn first_volume(
        &self,
        id: Id,
        bytes: u64,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> Result<(&DataVolume, Option<Intake<'_>>), Error> {
        for _ in 0..PLACEMENT_ATTEMPTS {
            let generation = self.placement.generation()?;
            let samples = self.qualified_capacity();

            // A concurrent first save can make the durable route decisive
            // during the native queries or as this provisional slot is issued.
            if let Some(volume) = self.routed(id)? {
                return Ok((volume, None));
            }
            let proposed =
                samples.and_then(|points| self.placement.reserve(generation, id, bytes, &points));
            if let Some(volume) = self.routed(id)? {
                let matching = proposed
                    .ok()
                    .flatten()
                    .filter(|lease| lease.token == volume.token);
                return Ok((volume, matching));
            }
            match proposed {
                Ok(Some(lease)) => {
                    let volume = self.enrolled(lease.token).ok_or_else(unenrolled_data)?;
                    return Ok((volume, Some(lease)));
                }
                Ok(None) => {}
                Err(error) => {
                    authorize()?;
                    return Err(error.into());
                }
            }
        }
        authorize()?;
        Err(io::Error::from(io::ErrorKind::WouldBlock).into())
    }

    fn qualified_capacity(&self) -> io::Result<Vec<Sample>> {
        let mut samples = Vec::new();
        let mut identity_failure = None;
        let mut other_failure = None;
        for volume in &self.volumes {
            match volume.available_bytes() {
                Ok(free) => samples.push((volume.token, free)),
                Err(error) => {
                    self.name_clock.fault();
                    let failure = match error.kind() {
                        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => {
                            &mut identity_failure
                        }
                        _ => &mut other_failure,
                    };
                    failure.get_or_insert(error);
                }
            }
        }
        if samples.is_empty() {
            // An earlier missing or read-only disk must not hide a later
            // held-identity refusal. Neither error substitutes another path.
            return Err(identity_failure
                .or(other_failure)
                .unwrap_or_else(unenrolled_data));
        }
        Ok(samples)
    }

    fn recycled(&self, id: Id) -> io::Result<Option<(RecycleReceipt, HeldDir)>> {
        self.name_observed(self.selected_recycle(id))
    }

    fn selected_recycle(&self, id: Id) -> io::Result<Option<(RecycleReceipt, HeldDir)>> {
        let Some(archive) = self.archive.as_ref() else {
            return Ok(None);
        };
        let selected = archive.selected(id)?;
        if let Some((receipt, _)) = &selected
            && receipt.is_enrolled()
        {
            let token = self
                .routes
                .get(id)?
                .ok_or_else(|| invalid("selected recycle record has no original DATA route"))?;
            receipt.require_data(token)?;
            // A configured DATA token is valid even if its held mount is now offline.
            // A token no longer in the roster requires an exact durable retirement.
            if self.enrolled(token).is_none() && !archive.retired_data(token)? {
                return Err(unenrolled_data());
            }
        }
        Ok(selected)
    }

    fn object(&self, id: Id) -> io::Result<Arc<Mutex<Option<State>>>> {
        if id == [0; 16] {
            return Err(invalid("zero container identity"));
        }
        let mut objects = self
            .objects
            .lock()
            .map_err(|_| invalid("container catalogue unavailable"))?;
        if let Some(active) = objects.get(&id) {
            return Ok(Arc::clone(active));
        }
        // Only the registry can issue a new strong reference. A slot owned
        // solely by the registry has no in-flight reader or writer.
        while objects.len() >= OBJECT_SLOT_TARGET {
            let idle = objects
                .iter()
                .find_map(|(id, slot)| (Arc::strong_count(slot) == 1).then_some(*id));
            let Some(id) = idle else { break };
            objects.remove(&id);
        }
        Ok(objects.entry(id).or_default().clone())
    }
    fn stage<'a>(&self, volume: &'a DataVolume) -> io::Result<FlatStage<'a>> {
        volume.require_named()?;
        self.flat_stage_at(volume.staging())
    }

    fn flat_stage_at<'a>(&self, root: &'a HeldDir) -> io::Result<FlatStage<'a>> {
        loop {
            let n = self
                .next_stage
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| invalid("staging name counter exhausted"))?;
            let name =
                CString::new(format!("{n:016x}")).map_err(|_| invalid("invalid stage name"))?;
            match FlatStage::new(root, name) {
                Ok(stage) => return Ok(stage),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// One data call validates and durably selects one complete node.
    /// Its body reader must end at this request. No payload parsing or second copy.
    pub(crate) fn save(
        &self,
        mutation: Mutation,
        change: Change,
        input: impl Read,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        let Change::Save { body, target, .. } = &change else {
            return Err(invalid("a node requires a complete save operation").into());
        };
        let first_body = matches!(target, Target::FirstMain { .. }).then_some(body.length);
        let (volume, _intake) = self.write_volume(mutation.container, first_body, &authorize)?;
        self.save_on(mutation, change, input, volume, authorize)
    }

    fn save_on(
        &self,
        mutation: Mutation,
        change: Change,
        mut input: impl Read,
        volume: &DataVolume,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        let Change::Save { body, .. } = &change else {
            return Err(invalid("a node requires a complete save operation").into());
        };
        let mut stage = self.stage(volume)?;
        {
            let file = stage.create(c"payload")?;
            body.copy_exact(&mut input, file, || Ok(()))?;
            file.sync_all()?;
        }
        self.publish(mutation, change, volume, &mut stage, authorize)
    }

    pub(crate) fn change(
        &self,
        mutation: Mutation,
        change: Change,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        if matches!(change, Change::Save { .. }) {
            return Err(invalid("a node must carry the original complete body").into());
        }
        let (volume, _intake) = self.write_volume(mutation.container, None, &authorize)?;
        self.publish(
            mutation,
            change,
            volume,
            &mut self.stage(volume)?,
            authorize,
        )
    }

    fn publish(
        &self,
        mutation: Mutation,
        change: Change,
        volume: &DataVolume,
        stage: &mut FlatStage<'_>,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        let id = mutation.container;
        let object = self.object(id)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        self.publish_under_gate(mutation, change, (volume, &mut memory), stage, authorize)
    }

    fn publish_under_gate(
        &self,
        mutation: Mutation,
        change: Change,
        (volume, memory): (&DataVolume, &mut Option<State>),
        stage: &mut impl Publication,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        let id = mutation.container;
        if self.recycled(id)?.is_some() {
            authorize()?;
            return Err(Conflict::MissingContainer.into());
        }
        self.name_observed(volume.require_named())?;
        let reserved = match self.name_observed(self.routes.get(id))? {
            Some(token) if token == volume.token => true,
            None if memory.is_none() => false,
            _ => {
                return self
                    .name_observed(Err(invalid("DATA route changed during intake")))
                    .map_err(Into::into);
            }
        };
        let mut first = State::default();
        let selected = if reserved {
            self.name_observed(volume.load(id, memory))?
        } else {
            &mut first
        };
        let pending = match selected.plan(mutation, change) {
            Ok(Decision::New(pending)) => pending,
            Ok(Decision::Replay(receipt)) => {
                authorize()?;
                return Ok(receipt);
            }
            Err(error) => {
                authorize()?;
                return Err(error.into());
            }
        };
        if let Err(error) =
            self.publish_new(pending.receipt(), (volume, reserved), stage, &authorize)
        {
            if matches!(error, Error::Uncertain(_)) {
                *memory = None;
            }
            return Err(error);
        }
        let receipt = pending.commit();
        if !reserved {
            *memory = Some(first);
        }
        Ok(receipt)
    }

    fn publish_new(
        &self,
        receipt: &Receipt,
        (volume, reserved): (&DataVolume, bool),
        stage: &mut impl Publication,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> Result<(), Error> {
        stage.stage_receipt(receipt, authorize)?;
        let mut bound = if let Some(label) = name_index::active_label(receipt) {
            let mut guard = self
                .names
                .lock()
                .map_err(|_| invalid("container name selection unavailable"))?;
            let Some(ticket) =
                guard.admit(self, receipt.mutation.container, label, volume.token)?
            else {
                authorize()?;
                return Err(Conflict::AlreadyExists.into());
            };
            if self.archive.is_some() {
                guard.preflight(receipt.mutation.container, volume.token, label.len())?;
            }
            guard.invalidate(); // Keep a prior witness; clear its fast grant before any write.
            Some((guard, ticket))
        } else {
            None
        };
        let ticket = bound.as_ref().map(|(_, ticket)| ticket);
        let (directory, changed) =
            self.publication_directory(receipt, (volume, reserved), stage, ticket, authorize)?;
        if receipt.sequence == 1 {
            self.name_observed(directory.only(None))?;
        }
        // Also covers a prior failed first publication of this directory.
        self.name_observed(volume.sync_data())?;
        let destination = receipt_name(receipt)?;
        self.name_observed(volume.require_named())?;
        self.name_observed(directory.require_named())?;
        stage.install(&directory, &destination, || {
            if let Some(ticket) = ticket {
                ticket.require(self, changed)?;
            }
            authorize()?;
            ticket.map_or(Ok(()), |proof| proof.require_generation(self))
        })?;
        if let Err(error) = directory
            .file
            .sync_all()
            .and_then(|()| volume.sync_staging())
            .and_then(|()| stage.confirm_moved(&directory, &destination))
        {
            self.name_clock.fault();
            return Err(Error::Uncertain(error));
        }
        // Projection is best effort after the authoritative native flush.
        if let Some((guard, ticket)) = bound.as_mut() {
            guard.confirm(self, ticket, changed, receipt, (&directory, volume.token));
        }
        Ok(())
    }

    fn publication_directory(
        &self,
        receipt: &Receipt,
        (volume, reserved): (&DataVolume, bool),
        stage: &impl Publication,
        ticket: Option<&Ticket>,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> Result<(HeldDir, ChangeCut), Error> {
        let id = receipt.mutation.container;
        let mut changed = ChangeCut::default();
        let cut = |changed| ticket.map_or(Ok(()), |proof| proof.require(self, changed));
        let checked = ticket.is_some_and(Ticket::held);
        let last = || ticket.map_or(Ok(()), |proof| proof.require_generation(self));
        if !reserved {
            self.name_observed(volume.require_vacant(id))?;
            cut(changed)?;
            let prove = || cut(ChangeCut::default().route());
            let check = checked.then_some(NameCut {
                prove: &prove,
                last: &last,
            });
            stage.reserve_first(&self.routes, id, volume.token, check, authorize)?;
            changed = changed.route();
        } else if let Some(original) = self.name_observed(volume.held_container(id))? {
            return Ok((original, changed));
        } else if receipt.sequence != 1 {
            return self
                .name_observed(Err(invalid("recorded DATA container is absent")))
                .map_err(Into::into);
        }
        // A fresh or interrupted first route still needs its own held directory.
        cut(changed)?;
        let prove = || cut(changed);
        let check = checked.then_some(NameCut {
            prove: &prove,
            last: &last,
        });
        let created = stage.create_first(volume, id, check, authorize)?;
        Ok((created, changed.data(volume.token)))
    }

    fn recorded(&self, id: Id) -> io::Result<&DataVolume> {
        self.routed(id)?
            .ok_or_else(|| invalid("selected container route is absent"))
    }

    fn vacant(&self, except: Id, name: &str) -> io::Result<bool> {
        #[cfg(test)]
        self.name_clock.count_routes();
        for id in self.name_observed(self.routes.ids())? {
            if id != except && self.recycled(id)?.is_none() {
                #[cfg(test)]
                self.name_clock.count_ledger();
                if self
                    .name_observed(self.recorded(id)?.read(id))?
                    .name
                    .as_deref()
                    == Some(name)
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    pub(crate) fn tags(&self, id: Id) -> io::Result<Option<Vec<String>>> {
        self.project_container(id, false, |state, _| state.tags.clone())
    }

    pub(crate) fn detail_page(
        &self,
        id: Id,
        sequence: Option<u64>,
        after: Option<&str>,
    ) -> Result<Option<DetailPage>, Error> {
        self.project_container(id, false, |state, name| {
            select_detail_page(id, name, state, sequence, after)
        })?
        .transpose()
    }

    fn project_container<T>(
        &self,
        id: Id,
        recorded: bool,
        project: impl FnOnce(&State, &str) -> T,
    ) -> io::Result<Option<T>> {
        let object = self.object(id)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        if self.recycled(id)?.is_some() {
            return Ok(None);
        }
        let volume = if recorded {
            self.recorded(id)?
        } else {
            let Some(volume) = self.routed(id)? else {
                return Ok(None);
            };
            volume
        };
        let state = self.name_observed(volume.load(id, &mut memory))?;
        Ok(state.name.as_deref().map(|name| project(state, name)))
    }
    pub(crate) fn catalog_page(
        &self,
        after: Option<Id>,
        readable: impl Fn(Id) -> bool,
    ) -> io::Result<CatalogPage> {
        catalog_rows(
            self.name_observed(self.routes.ids())?,
            after,
            readable,
            |id| {
                self.project_container(id, true, |state, name| CatalogRow {
                    id,
                    sequence: state.sequence,
                    name: name.to_owned(),
                })
            },
        )
    }

    /// Recycle an entire selected container, not a branch or a node.
    pub(crate) fn move_to_recycle(
        &self,
        mutation: Mutation,
        expected_sequence: u64,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<RecycleReceipt, Error> {
        let archive = self
            .archive
            .as_ref()
            .ok_or_else(|| invalid("no qualified recycle storage"))?;
        if mutation.request == [0; 16] {
            return Err(invalid("zero recycle request identity").into());
        }
        let object = self.object(mutation.container)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        authorize()?;
        if let Some((done, at)) = self.recycled(mutation.container)? {
            if (done.mutation, done.original_sequence) != (mutation, expected_sequence) {
                return Err(Conflict::RequestReused.into());
            }
            let _namespace_activity = self.name_background()?;
            if let Err(error) = self.finish_recycle(archive, done, &at, &authorize) {
                return Err(Error::CleanupPending {
                    receipt: Box::new(done),
                    error,
                });
            }
            return Ok(done);
        }
        let Some(volume) = self.routed(mutation.container)? else {
            return Err(Conflict::MissingContainer.into());
        };
        let state = self.name_observed(volume.load(mutation.container, &mut memory))?;
        if state.name.is_none() {
            return Err(Conflict::MissingContainer.into());
        }
        if state.receipts.contains_key(&mutation.request) {
            return Err(Conflict::RequestReused.into());
        }
        if state.sequence != expected_sequence {
            return Err(Conflict::SequenceChanged(state.sequence).into());
        }
        // Hold the exact DATA child across the copy and repeat both custody
        // and capacity at selection. A missing prior R can never select a new O.
        let source = self.name_observed(
            volume
                .held_container(mutation.container)
                .and_then(|at| at.ok_or_else(|| invalid("original recycle source is absent"))),
        )?;
        self.names
            .lock()
            .map_err(|_| invalid("container name selection unavailable"))?
            .preflight_fresh_recycle(mutation.container, (volume.token, &source), None)?;
        // Keep this guard until after both stages drop, and after source cleanup.
        let _namespace_activity = self.name_background()?;
        let mut incoming = self.flat_stage_at(archive.staging()?)?;
        let prepared = archive.prepare(mutation, volume, state, &mut incoming, || {
            authorize()?;
            volume.require_named()
        })?;
        let done = prepared.receipt;
        let mut selector = self.flat_stage_at(archive.moves()?)?;
        // Only one small SYSTEM rename switches logical visibility. The
        // ordinary namespace guard contains no streaming/hash work.
        {
            let mut names = self
                .names
                .lock()
                .map_err(|_| invalid("container name selection unavailable"))?;
            names.preflight_fresh_recycle(
                mutation.container,
                (volume.token, &source),
                Some(done),
            )?;
            archive.choose(&prepared, &mut selector, || {
                volume.require_named()?;
                let token = self
                    .routes
                    .get(mutation.container)?
                    .ok_or_else(|| invalid("selected recycle record has no original DATA route"))?;
                done.require_data(token)?;
                source.require_named()?;
                done.require_source(token, source.entity)?;
                authorize()
            })?;
            if let Err(error) = names.recycled(done, &prepared.at, volume.token) {
                return Err(Error::CleanupPending {
                    receipt: Box::new(done),
                    error,
                });
            }
        }
        if let Err(error) = archive.retire(
            done,
            volume,
            state,
            || Ok(()),
            || {
                prepared.at.require_named()?;
                authorize()
            },
        ) {
            return Err(Error::CleanupPending {
                receipt: Box::new(done),
                error,
            });
        }
        *memory = None;
        Ok(done)
    }

    fn finish_recycle(
        &self,
        archive: &recycle::Recycle,
        done: RecycleReceipt,
        at: &HeldDir,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        let id = done.mutation.container;
        let token = self
            .routes
            .get(id)?
            .ok_or_else(|| invalid("selected container route is absent"))?;
        done.require_data(token)?;
        // Explicit replay of a previously selected cut is just as final for
        // names as a fresh selector, even in a newly reopened Store.
        self.names
            .lock()
            .map_err(|_| invalid("container name selection unavailable"))?
            .recycled(done, at, token)?;
        let known = self.name_observed(read_state(at, id))?;
        archive.require_selected(done, &known)?;
        if let Some(volume) = self.enrolled(token) {
            return archive.retire(
                done,
                volume,
                &known,
                || {
                    if done.is_enrolled() {
                        recycle_material::verify_recovered(&known.receipts, at, || {
                            authorize()?;
                            volume.require_named()
                        })?;
                    }
                    Ok(())
                },
                || {
                    at.require_named()?;
                    authorize()
                },
            );
        }
        if !archive.retired_data(token)? {
            return Err(unenrolled_data());
        }
        authorize()
    }

    /// Verify the incoming continuation by walking back from this branch's
    /// exact still-selected head. Only the bounded page is cloned.
    pub(crate) fn branch_history(
        &self,
        id: Id,
        branch: &str,
        head: NodeId,
        next: Option<NodeId>,
    ) -> Result<Option<HistoryPage>, Error> {
        self.project_container(id, false, |state, _| {
            select_history_page(state, branch, head, next)
        })?
        .transpose()
    }

    pub(crate) fn node(&self, id: Id, node: NodeId) -> io::Result<Option<NodeBytes>> {
        self.name_observed(self.open_node(id, node, false))
    }

    pub(crate) fn node_for_payload(&self, id: Id, node: NodeId) -> io::Result<Option<NodeBytes>> {
        self.name_observed(self.open_node(id, node, true))
    }

    fn open_node(&self, id: Id, node: NodeId, exact_name: bool) -> io::Result<Option<NodeBytes>> {
        let object = self.object(id)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        if self.recycled(id)?.is_some() {
            return Ok(None);
        }
        let Some(volume) = self.routed(id)? else {
            return Ok(None);
        };
        let state = self.name_observed(volume.load(id, &mut memory))?;
        let Some(node) = state.nodes.get(&node).cloned() else {
            return Ok(None);
        };
        let container = volume
            .held_container(id)?
            .ok_or_else(|| invalid("selected DATA container is absent"))?;
        let position = entry_leaf(node.sequence, node.request);
        let entry = held_entry(&container, &position)?;
        let payload = entry
            .opaque_pin(c"payload")?
            .ok_or_else(|| invalid("selected node payload is absent"))?;
        if payload.file.metadata()?.len() != node.body.length {
            return Err(invalid(
                "complete node length differs from its durable receipt",
            ));
        }
        if exact_name {
            payload.require(&entry, c"payload")?;
            container.require_named()?;
            volume.require_named()?;
        }
        Ok(Some(NodeBytes {
            node,
            file: payload.file,
        }))
    }
}

fn receipt_name(receipt: &Receipt) -> io::Result<CString> {
    CString::new(entry_leaf(receipt.sequence, receipt.mutation.request))
        .map_err(|_| invalid("invalid container receipt name"))
}

fn entry_leaf(sequence: u64, request: Id) -> String {
    format!("{sequence:016x}-{}", hex(request))
}

fn held_entry(original: &HeldDir, leaf: &str) -> io::Result<HeldDir> {
    let native = CString::new(leaf).map_err(|_| invalid("invalid original receipt position"))?;
    original
        .child_directory(&native)?
        .ok_or_else(|| invalid("selected original node directory is absent"))
}

fn read_state(directory: &HeldDir, id: Id) -> io::Result<State> {
    read_confirmed(directory, || read_selected_entries(directory, id))
}

fn read_confirmed(
    directory: &HeldDir,
    read: impl FnOnce() -> io::Result<State>,
) -> io::Result<State> {
    directory.require_named()?;
    // Required before exposing any receipt/node after a prior process died.
    directory.file.sync_all()?;
    let state = read()?;
    directory.require_named()?;
    Ok(state)
}

fn read_selected_entries(directory: &HeldDir, id: Id) -> io::Result<State> {
    replay_selected_entries(directory, id, ReadAccess::Ordinary)
}

fn replay_selected_entries(directory: &HeldDir, id: Id, access: ReadAccess) -> io::Result<State> {
    replay_with(
        directory,
        id,
        access,
        (usize::MAX, usize::MAX),
        |_, bytes| codec::decode(bytes),
    )
}

fn ledger_limit() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        "selected ledger exceeds the bounded view",
    )
}

fn replay_with(
    directory: &HeldDir,
    id: Id,
    access: ReadAccess,
    (max_entries, mut remaining): (usize, usize),
    mut decode: impl FnMut(u64, &[u8]) -> io::Result<Receipt>,
) -> io::Result<State> {
    let mut entries = Vec::new();
    directory.each_name(access, |raw| {
        if entries.len() == max_entries {
            return Err(ledger_limit());
        }
        let name =
            std::str::from_utf8(raw).map_err(|_| invalid("invalid container receipt path"))?;
        let (sequence, request) = generation_name(name)?;
        entries.push((
            sequence,
            parse_container_id(request.as_bytes())?,
            name.to_owned(),
        ));
        Ok(())
    })?;
    entries.sort_unstable_by_key(|entry| entry.0);
    let mut state = State::default();
    for (sequence, request, name) in entries {
        let entry = held_entry(directory, &name)?;
        let original = entry
            .opaque_pin_with(c"receipt", access)?
            .ok_or_else(|| invalid("stored receipt is absent"))?;
        let mut bytes = Vec::new();
        let limit = remaining.min(codec::MAX_RECEIPT);
        (&original.file)
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)?;
        original.require(&entry, c"receipt")?;
        if bytes.len() > codec::MAX_RECEIPT {
            return Err(invalid("stored receipt exceeds metadata capacity"));
        }
        remaining = remaining
            .checked_sub(bytes.len())
            .ok_or_else(ledger_limit)?;
        let receipt = decode(sequence, &bytes)?;
        if receipt.mutation
            != (Mutation {
                container: id,
                request,
            })
            || receipt.sequence != sequence
        {
            return Err(invalid(
                "selected receipt differs from its recorded position",
            ));
        }
        state.recover(receipt).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid selected history: {e:?}"),
            )
        })?;
    }
    Ok(state)
}

fn qualify_roster(data: &mut [DataRoot]) -> io::Result<()> {
    data.sort_unstable_by_key(|volume| volume.token);
    if data.is_empty()
        || data[0].token == [0; 32]
        || data.windows(2).any(|pair| pair[0].token == pair[1].token)
    {
        return Err(invalid("invalid enrolled DATA roster"));
    }
    Ok(())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn unenrolled_data() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "selected DATA is not enrolled")
}
#[cfg(test)]
fn private_dir(path: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != local_uid() || m.mode() & 0o077 != 0 {
        return Err(invalid(
            "container storage directory must be private and service-owned",
        ));
    }
    Ok(())
}
#[cfg(test)]
fn ensure_dir(path: &Path) -> io::Result<bool> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            private_dir(path)?;
            Ok(false)
        }
        Err(e) => Err(e),
    }
}
#[cfg(test)]
fn create_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}
#[cfg(test)]
fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = create_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn hex<const N: usize>(id: [u8; N]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(2 * N);
    for b in id {
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 15)] as char);
    }
    out
}
fn parse_container_id(name: &[u8]) -> io::Result<Id> {
    if name.len() != 32 {
        return Err(invalid("invalid container directory identifier"));
    }
    let digit = |b| match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(invalid("invalid container directory identifier")),
    };
    let mut id = [0; 16];
    for (byte, pair) in id.iter_mut().zip(name.as_chunks::<2>().0) {
        *byte = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    if id == [0; 16] {
        return Err(invalid("invalid zero container directory identifier"));
    }
    Ok(id)
}
fn generation_name(leaf: &str) -> io::Result<(u64, &str)> {
    let (generation, request) = leaf
        .split_once('-')
        .ok_or_else(|| invalid("invalid selected container entry"))?;
    let lower_hex = |bytes: &[u8]| bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if generation.len() != 16
        || request.len() != 32
        || !lower_hex(generation.as_bytes())
        || !lower_hex(request.as_bytes())
    {
        return Err(invalid("invalid selected container entry"));
    }
    let generation =
        u64::from_str_radix(generation, 16).map_err(|_| invalid("invalid selected generation"))?;
    if generation == 0 {
        return Err(invalid("invalid selected generation"));
    }
    Ok((generation, request))
}
fn encode_tags(tags: &[String]) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for tag in tags {
        if tag.is_empty()
            || tag.contains('\0')
            || 4 + tag.len() > TAG_BYTES.saturating_sub(bytes.len())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid tag or excessive tag metadata",
            ));
        }
        let length = u32::try_from(tag.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "tag length exceeds its storage field",
            )
        })?;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(tag.as_bytes());
    }
    Ok(bytes)
}
fn decode_tags(
    mut bytes: &[u8],
    mut charge: impl FnMut() -> io::Result<()>,
) -> io::Result<Vec<String>> {
    let mut tags = Vec::new();
    while !bytes.is_empty() {
        let (length, suffix) = bytes
            .split_at_checked(4)
            .ok_or_else(|| invalid("truncated tag length"))?;
        let length = u32::from_be_bytes(
            length
                .try_into()
                .map_err(|_| invalid("truncated tag length"))?,
        ) as usize;
        let (text, rest) = suffix
            .split_at_checked(length)
            .ok_or_else(|| invalid("truncated tag"))?;
        let tag = std::str::from_utf8(text).map_err(|_| invalid("invalid tag encoding"))?;
        if tag.is_empty() || tag.contains('\0') {
            return Err(invalid("invalid stored tag"));
        }
        charge()?;
        tags.push(tag.to_owned());
        bytes = rest;
    }
    Ok(tags)
}

#[cfg(test)]
#[path = "../tests/unit/container_nodes.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/unit/resumable_wire.rs"]
mod resumable_wire;
