//! Bounded disposable membership streams, authenticated against the frozen State.
use super::*;
use crate::engine::history_window::HistoryCancellationCheckpoint;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

#[cfg(test)]
thread_local! {
    static READ_METRICS: Cell<([usize; 6], usize)> = const { Cell::new(([0;6],0)) };
}
#[cfg(test)]
pub(crate) fn reset_scope_read_metrics() {
    READ_METRICS.with(|metrics| metrics.set(([0; 6], 0)));
}
#[cfg(test)]
pub(crate) fn scope_read_metrics() -> ([usize; 6], usize) {
    READ_METRICS.with(Cell::get)
}

pub(super) const TABLE: &str = "CREATE TABLE history_scope_members(kind INTEGER NOT NULL,id TEXT NOT NULL COLLATE BINARY,entry_key TEXT NOT NULL COLLATE BINARY,file_id TEXT NOT NULL,history_tree_id TEXT NOT NULL,PRIMARY KEY(kind,id,entry_key))";
pub(super) const LOOKUP: &str = "CREATE INDEX history_scope_lookup ON history_scope_members(file_id,history_tree_id,kind,id,entry_key)";
type Counts = BTreeMap<(String, String), [usize; 6]>;

pub(crate) enum ScopeNode<'a> {
    Content(&'a FileVersion),
    Path(&'a FilePathNode),
    Metadata(&'a FileMetadataNode),
}

pub(super) struct Delta {
    keys: Option<[Vec<String>; 6]>,
}
fn changed<T: PartialEq>(
    before: Option<&TrackedRecords<T>>,
    after: &TrackedRecords<T>,
) -> Vec<String> {
    let Some(before) = before else {
        return after.keys().cloned().collect();
    };
    if before.shares_storage_with(after) {
        return Vec::new();
    }
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys
            .iter()
            .filter(|id| before.get(*id) != after.get(*id))
            .cloned()
            .collect(),
        MapChanges::All => {
            let mut keys = Vec::new();
            let mut old = before.iter().peekable();
            for (id, value) in after.iter() {
                while let Some((key, _)) = old.next_if(|(key, _)| *key < id) {
                    keys.push(key.clone());
                }
                let previous = old.next_if(|(key, _)| *key == id).map(|(_, v)| v);
                if previous != Some(value) {
                    keys.push(id.clone());
                }
            }
            keys.extend(old.map(|(key, _)| key.clone()));
            keys
        }
    }
}
fn changed_map<T: PartialEq>(
    before: Option<&storage_contract::TrackedMap<T>>,
    after: &storage_contract::TrackedMap<T>,
) -> Vec<String> {
    let Some(before) = before else {
        return after.keys().cloned().collect();
    };
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys
            .iter()
            .filter(|id| before.get(*id) != after.get(*id))
            .cloned()
            .collect(),
        MapChanges::All => {
            let mut keys = Vec::new();
            let mut old = before.iter().peekable();
            for (id, value) in after.iter() {
                while let Some((key, _)) = old.next_if(|(key, _)| *key < id) {
                    keys.push(key.clone());
                }
                let previous = old.next_if(|(key, _)| *key == id).map(|(_, v)| v);
                if previous != Some(value) {
                    keys.push(id.clone());
                }
            }
            keys.extend(old.map(|(key, _)| key.clone()));
            keys
        }
    }
}
impl Delta {
    pub(super) fn between(before: Option<&State>, after: &State) -> Self {
        Self {
            keys: before.map(|before| {
                [
                    changed(Some(&before.versions), &after.versions),
                    changed_map(Some(&before.file_path_nodes), &after.file_path_nodes),
                    changed_map(
                        Some(&before.file_metadata_nodes),
                        &after.file_metadata_nodes,
                    ),
                    changed(Some(&before.commits), &after.commits),
                    changed_map(Some(&before.branches), &after.branches),
                    changed_map(Some(&before.file_branch_merges), &after.file_branch_merges),
                ]
            }),
        }
    }
    fn keys<'a>(&'a self, state: &'a State, kind: usize) -> Box<dyn Iterator<Item = &'a str> + 'a> {
        if let Some(keys) = &self.keys {
            return Box::new(keys[kind].iter().map(String::as_str));
        }
        match kind {
            0 => Box::new(state.versions.keys().map(String::as_str)),
            1 => Box::new(state.file_path_nodes.keys().map(String::as_str)),
            2 => Box::new(state.file_metadata_nodes.keys().map(String::as_str)),
            3 => Box::new(state.commits.keys().map(String::as_str)),
            4 => Box::new(state.branches.keys().map(String::as_str)),
            5 => Box::new(state.file_branch_merges.keys().map(String::as_str)),
            _ => unreachable!("invariant: six membership kinds"),
        }
    }
    pub(super) async fn project(
        &self,
        c: &mut sqlite_access::SqliteConnection,
        after: &State,
    ) -> std::result::Result<(), ProjectionFailure> {
        for kind in 0..6 {
            for id in self.keys(after, kind) {
                if self.keys.is_some() {
                    sqlite_access::query("DELETE FROM history_scope_members WHERE kind=? AND id=?")
                        .bind(kind as i64)
                        .bind(id)
                        .execute(&mut *c)
                        .await?;
                }
                for (position, file, history) in members(after, kind, id) {
                    sqlite_access::query("INSERT INTO history_scope_members VALUES(?,?,?,?,?)")
                        .bind(kind as i64)
                        .bind(id)
                        .bind(position)
                        .bind(file)
                        .bind(history)
                        .execute(&mut *c)
                        .await?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn apply_counts(&self, counts: &mut Counts, before: Option<&State>, after: &State) {
        for kind in 0..6 {
            for id in self.keys(after, kind) {
                if let Some(before) = before {
                    for (_, file, history) in members(before, kind, id) {
                        let key = (file.to_owned(), history.to_owned());
                        let count = counts
                            .get_mut(&key)
                            .expect("invariant: trusted scope count includes previous member");
                        count[kind] -= 1;
                        if *count == [0; 6] {
                            counts.remove(&key);
                        }
                    }
                }
                for (_, file, history) in members(after, kind, id) {
                    counts
                        .entry((file.to_owned(), history.to_owned()))
                        .or_default()[kind] += 1;
                }
            }
        }
    }
}
fn members<'a>(
    state: &'a State,
    kind: usize,
    id: &str,
) -> impl Iterator<Item = (String, &'a str, &'a str)> {
    let node = match kind {
        0 => state
            .versions
            .get(id)
            .map(|v| (v.file_id.as_str(), v.history_tree_id.as_str())),
        1 => state
            .file_path_nodes
            .get(id)
            .map(|v| (v.file_id.as_str(), v.history_tree_id.as_str())),
        2 => state
            .file_metadata_nodes
            .get(id)
            .map(|v| (v.file_id.as_str(), v.history_tree_id.as_str())),
        3 => None,
        4 => state.branches.get(id).and_then(|b| {
            b.file_id
                .as_deref()
                .map(|file| (file, b.history_tree_id.as_str()))
        }),
        5 => state
            .file_branch_merges
            .get(id)
            .map(|merge| (merge.file_id.as_str(), merge.history_tree_id.as_str())),
        _ => unreachable!("invariant: six membership kinds"),
    };
    let commit = if kind == 3 {
        state.commits.get(id)
    } else {
        None
    };
    node.into_iter()
        .map(|(file, history)| (String::new(), file, history))
        .chain(commit.into_iter().flat_map(|c| {
            c.entries.iter().enumerate().map(move |(i, e)| {
                (
                    format!("{i:016x}"),
                    e.file_id.as_str(),
                    c.history_tree_id.as_str(),
                )
            })
        }))
}

pub(crate) struct ScopeReader<'state, 'connection> {
    state: &'state State,
    connection: RefCell<&'connection mut sqlite_access::SqliteConnection>,
    file: &'connection str,
    history: &'connection str,
    expected: [usize; 6],
    active: Cell<usize>,
    failure: RefCell<Option<Error>>,
}
impl<'state> ScopeReader<'state, '_> {
    pub(crate) fn versions(&self) -> impl Iterator<Item = Result<&'state FileVersion>> + '_ {
        Stream::new(self, 0, 0).map(|row| {
            row.map(|value| match value {
                Member::Node(ScopeNode::Content(version)) => version,
                _ => unreachable!("invariant: version stream kind range"),
            })
        })
    }
    pub(crate) fn nodes(&self) -> impl Iterator<Item = Result<ScopeNode<'state>>> + '_ {
        Stream::new(self, 0, 2).map(|row| {
            row.map(|value| match value {
                Member::Node(node) => node,
                Member::Entry(_, _) | Member::Branch(_) | Member::Merge(_) => {
                    unreachable!("invariant: node stream kind range")
                }
            })
        })
    }
    pub(crate) fn branches(&self) -> impl Iterator<Item = Result<&'state Branch>> + '_ {
        Stream::new(self, 4, 4).map(|row| {
            row.map(|member| match member {
                Member::Branch(branch) => branch,
                _ => unreachable!("invariant: branch stream kind range"),
            })
        })
    }
    pub(crate) fn merges(&self) -> impl Iterator<Item = Result<&'state FileBranchMerge>> + '_ {
        Stream::new(self, 5, 5).map(|row| {
            row.map(|member| match member {
                Member::Merge(merge) => merge,
                _ => unreachable!("invariant: merge stream kind range"),
            })
        })
    }
    pub(crate) fn commit_entries(
        &self,
    ) -> impl Iterator<Item = Result<(&'state Commit, &'state Entry)>> + '_ {
        Stream::new(self, 3, 3).map(|row| {
            row.map(|value| match value {
                Member::Entry(commit, entry) => (commit, entry),
                Member::Node(_) | Member::Branch(_) | Member::Merge(_) => {
                    unreachable!("invariant: entry stream kind range")
                }
            })
        })
    }
}
enum Member<'a> {
    Node(ScopeNode<'a>),
    Entry(&'a Commit, &'a Entry),
    Branch(&'a Branch),
    Merge(&'a FileBranchMerge),
}
type Key = (i64, String, String);
struct Stream<'reader, 'state, 'connection> {
    reader: &'reader ScopeReader<'state, 'connection>,
    low: i64,
    high: i64,
    prior: Key,
    rows: VecDeque<Key>,
    counts: [usize; 6],
    done: bool,
}
impl<'reader, 'state, 'connection> Stream<'reader, 'state, 'connection> {
    fn new(reader: &'reader ScopeReader<'state, 'connection>, low: i64, high: i64) -> Self {
        reader.active.set(reader.active.get() + 1);
        Self {
            reader,
            low,
            high,
            prior: (low - 1, String::new(), String::new()),
            rows: VecDeque::new(),
            counts: [0; 6],
            done: false,
        }
    }
    fn read(&mut self) -> Result<Option<Member<'state>>> {
        if self.rows.is_empty() {
            let mut c = self.reader.connection.borrow_mut();
            let rows = platform_io::sqlite::block_on_sqlite_connection(
                sqlite_access::query("SELECT kind,id,entry_key FROM history_scope_members INDEXED BY history_scope_lookup WHERE file_id=? AND history_tree_id=? AND kind>=? AND kind<=? AND (kind,id,entry_key)>(?,?,?) ORDER BY kind,id,entry_key LIMIT 64")
                    .bind(self.reader.file).bind(self.reader.history).bind(self.low).bind(self.high)
                    .bind(self.prior.0).bind(&self.prior.1).bind(&self.prior.2).fetch_all(&mut **c))?;
            #[cfg(test)]
            READ_METRICS.with(|metrics| {
                let (visits, max) = metrics.get();
                metrics.set((visits, max.max(rows.len())));
            });
            for row in rows {
                self.rows
                    .push_back((row.try_get(0)?, row.try_get(1)?, row.try_get(2)?));
            }
        }
        let Some(key) = self.rows.pop_front() else {
            for kind in self.low..=self.high {
                if self.counts[kind as usize] != self.reader.expected[kind as usize] {
                    return Err(invalid("incomplete scope membership stream"));
                }
            }
            self.done = true;
            self.reader.active.set(self.reader.active.get() - 1);
            return Ok(None);
        };
        if key <= self.prior || key.0 < self.low || key.0 > self.high {
            return Err(invalid("invalid scope membership order"));
        }
        let state = self.reader.state;
        let (member, file, history, actual_id) = match key.0 {
            0 => {
                let v = state
                    .versions
                    .get(&key.1)
                    .ok_or_else(|| invalid("unknown scope version"))?;
                (
                    Member::Node(ScopeNode::Content(v)),
                    v.file_id.as_str(),
                    v.history_tree_id.as_str(),
                    v.version_id.as_str(),
                )
            }
            1 => {
                let v = state
                    .file_path_nodes
                    .get(&key.1)
                    .ok_or_else(|| invalid("unknown scope path node"))?;
                (
                    Member::Node(ScopeNode::Path(v)),
                    v.file_id.as_str(),
                    v.history_tree_id.as_str(),
                    v.node_id.as_str(),
                )
            }
            2 => {
                let v = state
                    .file_metadata_nodes
                    .get(&key.1)
                    .ok_or_else(|| invalid("unknown scope metadata node"))?;
                (
                    Member::Node(ScopeNode::Metadata(v)),
                    v.file_id.as_str(),
                    v.history_tree_id.as_str(),
                    v.node_id.as_str(),
                )
            }
            3 => {
                let c = state
                    .commits
                    .get(&key.1)
                    .ok_or_else(|| invalid("unknown scope commit"))?;
                let index = usize::from_str_radix(&key.2, 16)
                    .map_err(|_| invalid("invalid scope entry position"))?;
                if format!("{index:016x}") != key.2 {
                    return Err(invalid("noncanonical scope entry position"));
                }
                let e = c
                    .entries
                    .get(index)
                    .ok_or_else(|| invalid("unknown scope commit entry"))?;
                (
                    Member::Entry(c, e),
                    e.file_id.as_str(),
                    c.history_tree_id.as_str(),
                    c.commit_id.as_str(),
                )
            }
            4 => {
                let branch = state
                    .branches
                    .get(&key.1)
                    .ok_or_else(|| invalid("unknown scope branch"))?;
                let file = branch
                    .file_id
                    .as_deref()
                    .ok_or_else(|| invalid("non-file scope branch"))?;
                (
                    Member::Branch(branch),
                    file,
                    branch.history_tree_id.as_str(),
                    branch.branch_id.as_str(),
                )
            }
            5 => {
                let merge = state
                    .file_branch_merges
                    .get(&key.1)
                    .ok_or_else(|| invalid("unknown scope merge"))?;
                (
                    Member::Merge(merge),
                    merge.file_id.as_str(),
                    merge.history_tree_id.as_str(),
                    merge.merge_id.as_str(),
                )
            }
            _ => unreachable!("invariant: validated kind range"),
        };
        if file != self.reader.file
            || history != self.reader.history
            || actual_id != key.1
            || (key.0 != 3 && !key.2.is_empty())
        {
            return Err(invalid("scope membership disagrees with State"));
        }
        #[cfg(test)]
        READ_METRICS.with(|metrics| {
            let (mut visits, max) = metrics.get();
            visits[key.0 as usize] += 1;
            metrics.set((visits, max));
        });
        self.counts[key.0 as usize] += 1;
        if self.counts[key.0 as usize] > self.reader.expected[key.0 as usize] {
            return Err(invalid("excess scope membership"));
        }
        self.prior = key;
        Ok(Some(member))
    }
}
impl<'state> Iterator for Stream<'_, 'state, '_> {
    type Item = Result<Member<'state>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.read() {
            Ok(value) => value.map(Ok),
            Err(error) => {
                self.done = true;
                let detail = error.to_string();
                let mut failure = self.reader.failure.borrow_mut();
                if failure.is_none() {
                    *failure = Some(error);
                }
                Some(Err(invalid(detail)))
            }
        }
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidRequest, message.into())
}

impl Engine {
    pub(crate) fn indexed_history_owner(
        &self,
        file: &str,
        cancel: &storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<Option<&FileVersion>> {
        cancel.checkpoint()?;
        let scopes = {
            let proof = self
                .history_query_proof
                .lock()
                .expect("invariant: history query proof lock poisoned");
            if !matches!(proof.availability, IndexAvailability::Ready(_)) {
                return Ok(None);
            }
            proof
                .scope_counts
                .range((file.to_owned(), String::new())..)
                .take_while(|((candidate, _), _)| candidate == file)
                .filter(|(_, counts)| counts[0] != 0)
                .map(|((_, history), _)| history.clone())
                .collect::<Vec<_>>()
        };
        let mut owner: Option<&FileVersion> = None;
        for history in scopes {
            cancel.checkpoint()?;
            let Some(scope_owner) = self.with_history_scope(file, &history, |reader| {
                let mut first: Option<&FileVersion> = None;
                for version in reader.versions() {
                    cancel.checkpoint()?;
                    let version = version?;
                    if first.is_none_or(|old| version.version_id < old.version_id) {
                        first = Some(version);
                    }
                }
                Ok(first)
            })?
            else {
                return Ok(None);
            };
            if let Some(version) = scope_owner
                && owner.is_none_or(|old| version.version_id < old.version_id)
            {
                owner = Some(version);
            }
        }
        Ok(owner)
    }
    pub(crate) fn with_history_scope<'state, T>(
        &'state self,
        file: &str,
        history: &str,
        callback: impl FnOnce(&ScopeReader<'state, '_>) -> Result<T>,
    ) -> Result<Option<T>> {
        let Some(snapshot) = self.capture_history_scope(file, history)? else {
            return Ok(None);
        };
        let result = snapshot.with_state(&self.state, callback);
        self.record_scope_failure(&snapshot);
        result
    }
}

/// Separate projection failures from callback business errors and cancellation.
pub(super) fn execute<'state, T>(
    state: &'state State,
    c: &mut sqlite_access::SqliteConnection,
    file: &str,
    history: &str,
    expected: [usize; 6],
    callback: impl FnOnce(&ScopeReader<'state, '_>) -> Result<T>,
) -> Result<Result<T>> {
    let reader = ScopeReader {
        state,
        connection: RefCell::new(c),
        file,
        history,
        expected,
        active: Cell::new(0),
        failure: RefCell::new(None),
    };
    let output = callback(&reader);
    if let Some(error) = reader.failure.into_inner() {
        return Err(error);
    }
    match output {
        Err(error) => {
            if let Err(cleanup) =
                platform_io::sqlite::block_on_sqlite_connection(c.execute("ROLLBACK"))
            {
                return Err(preserve_close_failure(error, cleanup.into()));
            }
            Ok(Err(error))
        }
        Ok(output) => {
            if reader.active.get() != 0 {
                return Err(invalid("scope stream was not completely consumed"));
            }
            platform_io::sqlite::block_on_sqlite_connection(c.execute("COMMIT"))?;
            Ok(Ok(output))
        }
    }
}

pub(super) async fn check_schema(
    c: &mut sqlite_access::SqliteConnection,
) -> std::result::Result<(), ProjectionFailure> {
    for (name, kind, ddl) in [
        ("history_scope_members", "table", TABLE),
        ("history_scope_lookup", "index", LOOKUP),
    ] {
        let row = sqlite_access::query("SELECT type,sql FROM sqlite_schema WHERE name=?")
            .bind(name)
            .fetch_one(&mut *c)
            .await?;
        if row.try_get::<String, _>(0)? != kind || row.try_get::<String, _>(1)? != ddl {
            return Err("scope schema mismatch".into());
        }
    }
    Ok(())
}
