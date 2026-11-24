//! Local mappings, frozen unconfirmed payloads, and verified R3 receipt reconciliation.
use crate::{
    Result, bundle, invalid,
    native::{self, Folder, Lease, Source, Stamp},
    r3::{Body, Branch, Catalog, Detail, DisplayName, Id, Intent, Lookup, Target},
    session::{
        BasicRequests, ByteWindow, ExistingBasicLogin, ExistingBasicSession, PayloadError,
        PayloadSelection, PayloadSession, Principal, ReceiptSession, Session,
    },
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, Permissions},
    io::sink,
    os::unix::fs::{FileExt, MetadataExt, PermissionsExt},
    path::Path,
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

mod browse;
mod checkout;
pub use checkout::CheckoutChoice;

const DB: &str = "workspace.sqlite";
const DB_SIDECARS: [&str; 3] = [
    "workspace.sqlite-journal",
    "workspace.sqlite-wal",
    "workspace.sqlite-shm",
];
const APP: i64 = 0x5a58_4331;
const VERSION: i64 = 1;
const SCHEMA: &str = "
CREATE TABLE profile(id INTEGER PRIMARY KEY CHECK(id=1), record TEXT NOT NULL) STRICT;
CREATE TABLE basis(path TEXT PRIMARY KEY, record TEXT NOT NULL) STRICT, WITHOUT ROWID;
CREATE TABLE original(op TEXT PRIMARY KEY, container TEXT NOT NULL, path TEXT NOT NULL,
  record TEXT NOT NULL, phase TEXT NOT NULL CHECK(phase IN ('prepared','attempted','unknown','absent','conflict','confirmed')),
  receipt TEXT, cleaned INTEGER NOT NULL DEFAULT 0 CHECK(cleaned IN (0,1))) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX one_path ON original(path) WHERE phase != 'confirmed';
CREATE UNIQUE INDEX one_container ON original(container) WHERE phase != 'confirmed';
CREATE TABLE bundle_manifest(path TEXT PRIMARY KEY, record TEXT NOT NULL) STRICT, WITHOUT ROWID;
";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Basis {
    pub container: Id,
    pub branch: Branch,
    pub node: Id,
    pub seen_sequence: u64,
    pub body: Body,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {
    intent: Intent,
    path: String,
    prior: Option<Basis>,
    sealed: Stamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolution {
    pub operation: Id,
    pub status: Lookup,
    pub local_cleanup_pending: bool,
}

pub struct Workspace {
    root: Folder,
    db: Connection,
    db_id: (u64, u64),
    principal: Principal,
    read_session_owner: Arc<()>,
}

#[derive(Clone, Copy)]
enum BasicLane<'a> {
    Legacy(&'a Session),
    Existing(&'a ExistingBasicSession),
}

impl<'a> BasicLane<'a> {
    fn requests<F: Fn() -> Result<()>>(self, before_wire: F) -> BasicRequests<'a, F> {
        match self {
            Self::Legacy(session) => session.requests(before_wire),
            Self::Existing(session) => session.requests(before_wire),
        }
    }
}

enum ReceiptLane<'a> {
    Basic(BasicLane<'a>),
    Receipt(&'a ReceiptSession),
}

/// Existing private workspace opened without SQLite write permission.
pub struct PayloadWorkspace(Workspace, Source);

/// Exact original confirmed reference and held ordinary destination.
pub struct PayloadSlice {
    owner: Arc<()>,
    mapped: String,
    selected: Basis,
    selection: PayloadSelection,
    output: String,
    parent: Folder,
    leaf: OsString,
}

/// One local cancellation review. This never owns a transport or authorizes
/// removal of a working file, confirmed basis, or possibly submitted original.
pub struct PreparedCancellation {
    owner: Arc<()>,
    original: Original,
    working: Source,
}

impl PreparedCancellation {
    pub fn intent(&self) -> &Intent {
        &self.original.intent
    }

    pub fn working_path(&self) -> &str {
        &self.original.path
    }
}

pub struct Review {
    source: Source,
    path: String,
    prior: Option<Basis>,
    first: Option<DisplayName>,
    body: Body,
}

impl Review {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn body(&self) -> &Body {
        &self.body
    }
    pub fn basis(&self) -> Option<&Basis> {
        self.prior.as_ref()
    }
    pub fn name(&self) -> Option<&DisplayName> {
        self.first.as_ref()
    }
    pub fn changed(&self) -> bool {
        self.prior.as_ref().is_none_or(|p| p.body != self.body)
    }
}

impl Workspace {
    pub fn initialize(directory: &Path, principal: &Principal) -> Result<Self> {
        let root = Folder::open(directory, true)?;
        if !root.names(Path::new(""))?.is_empty() {
            return Err(invalid(
                "an explicitly chosen independent empty 0700 directory is required",
            ));
        }
        let bin = root.new_private(DB)?;
        bin.sync_all()?;
        root.sync()?;
        drop(bin);
        let mut opened = Self::connect(root, Some(principal))?;
        let transaction = opened
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(SCHEMA)?;
        transaction.pragma_update(None, "application_id", APP)?;
        transaction.pragma_update(None, "user_version", VERSION)?;
        transaction.execute(
            "INSERT INTO profile VALUES(1,?1)",
            [serde_json::to_string(principal)?],
        )?;
        transaction.commit()?;
        opened.root.sync()?;
        opened.validate()?;
        Ok(opened)
    }

    pub fn open(directory: &Path, principal: &Principal) -> Result<Self> {
        let opened = Self::open_local(directory)?;
        if &opened.principal != principal {
            return Err(invalid("original local trust or account did not match"));
        }
        Ok(opened)
    }

    pub fn open_local(directory: &Path) -> Result<Self> {
        let opened = Self::connect(Folder::open(directory, true)?, None)?;
        opened.validate()?;
        Ok(opened)
    }

    fn connect(root: Folder, enrolled: Option<&Principal>) -> Result<Self> {
        let (opened, _original_file) =
            Self::connect_with_access(root, enrolled, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        Ok(opened)
    }

    fn connect_with_access(
        root: Folder,
        enrolled: Option<&Principal>,
        access: OpenFlags,
    ) -> Result<(Self, Source)> {
        let db_file = root.source(Path::new(DB))?;
        if db_file.file.metadata()?.mode() & 0o777 != 0o600 {
            return Err(invalid("workspace SQLite file is not private 0600"));
        }
        let payload_reader = access.contains(OpenFlags::SQLITE_OPEN_READ_ONLY);
        if enrolled.is_some() {
            if db_file.before.length != 0 {
                return Err(invalid("refusing to initialize a nonempty SQLite file"));
            }
        } else if !payload_reader {
            check_workspace_header(&db_file, false)?;
        }
        let db_id = (db_file.before.dev, db_file.before.ino);
        let open = || -> Result<_> {
            let db = Connection::open_with_flags(
                root.path().join(DB),
                access | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )?;
            db.busy_timeout(Duration::from_secs(5))?;
            db.pragma_update(None, "trusted_schema", "OFF")?;
            if access.contains(OpenFlags::SQLITE_OPEN_READ_WRITE) {
                db.pragma_update(None, "synchronous", "EXTRA")?;
            }
            let mode: String = db.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
            if mode != "delete" {
                return Err(invalid("unexpected independent SQLite journal mode"));
            }
            let principal = match enrolled {
                Some(value) => value.clone(),
                None => {
                    let (app, version) = schema_version(&db)?;
                    if (app, version) != (APP, VERSION) {
                        return Err(invalid("not independent ZXS Rust client state"));
                    }
                    let profile: String =
                        db.query_row("SELECT record FROM profile WHERE id=1", [], |r| r.get(0))?;
                    serde_json::from_str::<Principal>(&profile)?
                }
            };
            Ok((db, principal))
        };
        let (db, principal) = if payload_reader {
            inspect_payload_database(&root, &db_file, open)?
        } else {
            open()?
        };
        let opened = Self {
            root,
            db,
            db_id,
            principal,
            read_session_owner: Arc::new(()),
        };
        opened.held_database()?;
        Ok((opened, db_file))
    }

    fn held_database(&self) -> Result<()> {
        held_database_at(&self.root, self.db_id)
    }

    fn hold_container(&self, container: &Id) -> Result<Lease> {
        self.held_database()?;
        self.root.lock(&format!("lease-{container}"))
    }

    fn validate(&self) -> Result<()> {
        self.held_database()?;
        require_profile(&self.db, &self.principal)?;
        self.held_database()
    }

    fn validate_basic_lane(&self, lane: BasicLane<'_>) -> Result<()> {
        self.validate()?;
        let same = match lane {
            BasicLane::Legacy(session) => session.principal() == &self.principal,
            BasicLane::Existing(session) => {
                session.belongs_to(&self.read_session_owner, &self.principal)
            }
        };
        if !same {
            return Err(invalid(
                "the Basic session does not match this original workspace",
            ));
        }
        Ok(())
    }

    /// Obtain one verified channel before the shell asks for a password.
    pub async fn begin_existing_basic(
        &self,
        spelling: &str,
        account: &str,
    ) -> Result<ExistingBasicLogin> {
        self.validate()?;
        let ready = ExistingBasicLogin::begin(
            &self.principal,
            Arc::clone(&self.read_session_owner),
            spelling,
            account,
        )
        .await?;
        self.validate()?;
        Ok(ready)
    }

    /// Consume this exact verified channel after the response to the password prompt.
    pub fn finish_existing_basic(
        &self,
        ready: ExistingBasicLogin,
        password: Zeroizing<String>,
    ) -> Result<ExistingBasicSession> {
        self.validate()?;
        ready.finish(&self.read_session_owner, &self.principal, password)
    }

    pub async fn catalog_existing(
        &self,
        session: &ExistingBasicSession,
        after: Option<&Id>,
    ) -> Result<Catalog> {
        let guard = || self.validate_basic_lane(BasicLane::Existing(session));
        guard()?;
        session.requests(&guard).catalog(after).await
    }

    pub async fn detail_existing(
        &self,
        session: &ExistingBasicSession,
        container: &Id,
    ) -> Result<Detail> {
        let guard = || self.validate_basic_lane(BasicLane::Existing(session));
        guard()?;
        session.requests(&guard).detail(container).await
    }

    /// Explicit receipt-only authentication for this already-open workspace.
    /// The prompt is not called before the exact workspace and TLS/H2 checks.
    pub async fn connect_receipts(
        &self,
        spelling: &str,
        account: &str,
        prompt: impl FnOnce() -> Result<Zeroizing<String>>,
    ) -> Result<ReceiptSession> {
        self.validate()?;
        ReceiptSession::connect_bound(
            &self.principal,
            Arc::clone(&self.read_session_owner),
            spelling,
            account,
            prompt,
            || self.validate(),
        )
        .await
    }

    fn validate_receipt_lane(&self, lane: &ReceiptLane<'_>) -> Result<()> {
        match lane {
            ReceiptLane::Basic(lane) => self.validate_basic_lane(*lane),
            ReceiptLane::Receipt(session) => {
                self.validate()?;
                if !session.belongs_to(&self.read_session_owner, &self.principal) {
                    return Err(invalid(
                        "the receipt session has a different original workspace",
                    ));
                }
                session.require_live()
            }
        }
    }

    fn normal_path<'a>(&self, path: &'a Path) -> Result<&'a str> {
        let text = native::literal(path)?;
        if path.starts_with(self.root.path()) || self.root.path().starts_with(path) {
            return Err(invalid(
                "private client state is not an ordinary editable workspace",
            ));
        }
        Ok(text)
    }

    pub fn basis(&self, path: &Path) -> Result<Option<Basis>> {
        self.validate()?;
        let text = self.normal_path(path)?;
        db_basis(&self.db, text)
    }

    pub fn review(&self, path: &Path, first_name: Option<DisplayName>) -> Result<Review> {
        let text = self.normal_path(path)?;
        let prior = self.basis(path)?;
        if prior.is_some() == first_name.is_some() {
            return Err(invalid(
                "choose an existing binding or a display name for this first full file",
            ));
        }
        let place = Folder::open(
            path.parent().ok_or_else(|| invalid("file has no parent"))?,
            false,
        )?;
        let source = place.source(Path::new(
            path.file_name()
                .ok_or_else(|| invalid("file has no leaf"))?,
        ))?;
        let body = source.scan(sink())?;
        Ok(Review {
            source,
            path: text.to_owned(),
            prior,
            first: first_name,
            body,
        })
    }

    pub fn prepare(&mut self, review: &Review, confirmed: bool) -> Result<Option<Intent>> {
        if !confirmed {
            return Err(invalid(
                "confirm this exact full-file review before preparing",
            ));
        }
        self.validate()?;
        self.normal_path(Path::new(&review.path))?;
        if self.basis(Path::new(&review.path))? != review.prior
            || review.source.scan(sink())? != review.body
        {
            return Err(invalid("the reviewed file or original binding changed"));
        }
        if !review.changed() {
            return Ok(None);
        }
        let (container_id, seen_sequence, target) = match (&review.prior, &review.first) {
            (Some(prior), None) => (
                prior.container.clone(),
                prior.seen_sequence,
                Target::Branch {
                    branch: prior.branch.clone(),
                    expected_head: prior.node.clone(),
                },
            ),
            (None, Some(name)) => (Id::fresh()?, 0, Target::FirstMain { name: name.clone() }),
            _ => return Err(invalid("invalid original full-file review")),
        };
        let intent = Intent {
            container_id,
            operation_id: Id::fresh()?,
            node_id: Id::fresh()?,
            seen_sequence,
            target,
            body: review.body.clone(),
        };
        intent.validate()?;
        let lease = self.hold_container(&intent.container_id)?;
        if unresolved_selection(&self.db, &intent.container_id, &review.path)? {
            return Err(invalid(
                "an unresolved original already owns this file or container",
            ));
        }
        let mut pending = self.root.unlinked()?;
        if review.source.scan(&mut pending)? != review.body {
            return Err(invalid("source changed after review"));
        }
        pending.set_permissions(Permissions::from_mode(0o400))?;
        self.retain_prepared(review, intent, &pending, &lease)
            .map(Some)
    }

    fn retain_prepared(
        &mut self,
        review: &Review,
        intent: Intent,
        pending: &File,
        lease: &Lease,
    ) -> Result<Intent> {
        let root = &self.root;
        let db_id = self.db_id;
        let recheck = || held_lease_at(root, db_id, lease);
        recheck()?;
        let name = payload(&intent.operation_id);
        if let Err(error) = root.publish_guarded(name.as_ref(), pending, &intent.body, recheck) {
            return Err(format!("original {} may have local staged bytes; no network sent, retain its evidence: {error}",
                               intent.operation_id).into());
        }
        let sealed = native::stamp(&pending.metadata()?);
        let retained = Original {
            intent: intent.clone(),
            path: review.path.clone(),
            prior: review.prior.clone(),
            sealed,
        };
        let commit = (|| -> Result<()> {
            recheck()?;
            let tx = self
                .db
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if db_basis(&tx, &review.path)? != review.prior
                || unresolved_selection(&tx, &intent.container_id, &review.path)?
            {
                return Err(invalid(
                    "the original local mapping or pending slot changed",
                ));
            }
            recheck()?;
            tx.execute("INSERT INTO original(op,container,path,record,phase) VALUES(?1,?2,?3,?4,'prepared')",
                       params![intent.operation_id.as_str(), intent.container_id.as_str(), &review.path,
                               serde_json::to_string(&retained)?])?;
            recheck()?;
            tx.commit()?;
            root.sync()?;
            recheck()
        })();
        if let Err(error) = commit {
            return Err(format!("local intent {} may have registered staging; no network sent; retain and reopen same original: {error}",
                               intent.operation_id).into());
        }
        Ok(intent)
    }

    pub fn pending(&self) -> Result<Vec<Id>> {
        self.validate()?;
        let mut sql = self
            .db
            .prepare("SELECT op FROM original WHERE phase!='confirmed' ORDER BY op")?;
        let rows = sql.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|value| Id::parse(&value?)).collect()
    }

    pub fn intent(&self, op: &Id) -> Result<Intent> {
        self.validate()?;
        Ok(self.original(op)?.0.intent)
    }

    /// Show the exact unarmed local candidate without modifying it or connecting.
    pub fn review_prepared_cancellation(&self, op: &Id) -> Result<PreparedCancellation> {
        self.validate()?;
        let (original, phase, _) = self.original(op)?;
        if phase != "prepared" {
            return Err(invalid(
                "only an original known never armed can be canceled offline",
            ));
        }
        let path = Path::new(&original.path);
        let folder = Folder::open(
            path.parent()
                .ok_or_else(|| invalid("working file has no original parent"))?,
            false,
        )?;
        // The current ordinary file must still exist, but can contain later edits.
        let working = folder.source(Path::new(
            path.file_name()
                .ok_or_else(|| invalid("working file has no original name"))?,
        ))?;
        if (working.before.dev, working.before.ino) == (original.sealed.dev, original.sealed.ino) {
            return Err(invalid(
                "the working file must be separate from the frozen copy",
            ));
        }
        let choice = PreparedCancellation {
            owner: Arc::clone(&self.read_session_owner),
            original,
            working,
        };
        self.check_prepared_cancellation(&choice)?;
        Ok(choice)
    }

    fn check_prepared_cancellation(&self, choice: &PreparedCancellation) -> Result<()> {
        if !Arc::ptr_eq(&self.read_session_owner, &choice.owner) {
            return Err(invalid(
                "the cancellation belongs to a different workspace choice",
            ));
        }
        self.validate()?;
        let (current, phase, _) = self.original(&choice.original.intent.operation_id)?;
        if phase != "prepared"
            || current != choice.original
            || db_basis(&self.db, &choice.original.path)? != choice.original.prior
        {
            return Err(invalid(
                "the exact never-armed original or its working basis changed; nothing authorized",
            ));
        }
        choice.working.check()
    }

    /// Discard only the explicitly confirmed never-armed frozen candidate.
    /// No readback, retry, new plan, unbinding, or working-file mutation.
    pub fn cancel_prepared(&self, choice: PreparedCancellation, confirmed: bool) -> Result<()> {
        if !confirmed {
            return Err(invalid(
                "explicitly confirm discarding this exact local frozen candidate",
            ));
        }
        self.check_prepared_cancellation(&choice)?;
        let original = &choice.original;
        let op = &original.intent.operation_id;
        let lease = self.hold_container(&original.intent.container_id)?;
        let guard = || {
            self.check_prepared_cancellation(&choice)?;
            held_lease_at(&self.root, self.db_id, &lease)
        };
        guard()?;
        let leaf = payload(op);
        if self.root.absent(leaf.as_ref()).is_err() {
            // Another inode, unverified data, or stale custody is never removed.
            self.root.remove_verified(&leaf, &original.intent.body, original.sealed, guard)
                .map_err(|error| format!(
                    "local cancellation of {op} was not completed; check its frozen copy, keep the working file: {error}",
                ))?;
        }
        // Namespace durability precedes removing the local intent. After a crash
        // the never-armed row may still be listed but no body is available to send.
        let finish = || -> Result<()> {
            self.root.sync()?;
            guard()?;
            self.root.absent(leaf.as_ref())?;
            if self.db.execute(
                "DELETE FROM original WHERE op=?1 AND record=?2 AND phase='prepared' AND receipt IS NULL",
                params![op.as_str(), serde_json::to_string(original)?],
            )? != 1
            {
                return Err(invalid("the never-armed original row changed before cancellation"));
            }
            held_lease_at(&self.root, self.db_id, &lease)
        };
        finish().map_err(|error| format!(
            "frozen copy for {op} may be gone but local cancellation was not verified; reopen its original state, never resend: {error}",
        ).into())
    }

    fn original(&self, op: &Id) -> Result<(Original, String, Option<u64>)> {
        self.held_database()?;
        let (cid, stored_path, raw, phase, receipt): (
            String,
            String,
            String,
            String,
            Option<String>,
        ) = self.db.query_row(
            "SELECT container,path,record,phase,receipt FROM original WHERE op=?1",
            [op.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        let original: Original = serde_json::from_str(&raw)?;
        self.normal_path(Path::new(&original.path))?;
        if &original.intent.operation_id != op
            || original.intent.container_id.as_str() != cid
            || original.path != stored_path
        {
            return Err(invalid(
                "the retained original does not match the exact requested operation",
            ));
        }
        original.intent.validate()?;
        let sequence = receipt.map(|s| s.parse::<u64>()).transpose()?;
        if (phase == "confirmed") != sequence.is_some()
            || sequence.is_some_and(|n| {
                n <= original.intent.seen_sequence
                    || n == u64::MAX
                    || (original.intent.seen_sequence == 0 && n != 1)
            })
        {
            return Err(invalid("original local receipt record disagrees"));
        }
        Ok((original, phase, sequence))
    }

    /// Retry only local cleanup of independently confirmed receipts; never has a transport handle.
    pub fn cleanup_on_open(&self) -> Result<Vec<Resolution>> {
        self.validate()?;
        let mut sql = self
            .db
            .prepare("SELECT op FROM original WHERE phase='confirmed' AND cleaned=0 ORDER BY op")?;
        let entries: Vec<String> = sql
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        drop(sql);
        let mut outcomes = Vec::new();
        for op in entries {
            let operation = Id::parse(&op)?;
            let (original, _, receipt) = self.original(&operation)?;
            let Some(sequence) = receipt else { continue };
            let lease = self.hold_container(&original.intent.container_id)?;
            let local_cleanup_pending = !self.cleanup(&original, &lease)?;
            outcomes.push(Resolution {
                operation,
                status: Lookup::Confirmed { sequence },
                local_cleanup_pending,
            });
        }
        Ok(outcomes)
    }

    // Called when a shell connects. This loop can only send operation GETs, even after authoritative absence.
    pub async fn reconcile_on_open(&mut self, session: &Session) -> Result<Vec<Resolution>> {
        self.reconcile_receipt_lane(ReceiptLane::Basic(BasicLane::Legacy(session)))
            .await
    }

    pub async fn reconcile_existing(
        &mut self,
        session: &ExistingBasicSession,
    ) -> Result<Vec<Resolution>> {
        self.reconcile_receipt_lane(ReceiptLane::Basic(BasicLane::Existing(session)))
            .await
    }

    pub async fn reconcile_receipts(
        &mut self,
        session: &ReceiptSession,
    ) -> Result<Vec<Resolution>> {
        self.reconcile_receipt_lane(ReceiptLane::Receipt(session))
            .await
    }

    async fn reconcile_receipt_lane(&mut self, lane: ReceiptLane<'_>) -> Result<Vec<Resolution>> {
        self.validate_receipt_lane(&lane)?;
        let mut results = self.cleanup_on_open()?;
        for op in self.pending()? {
            results.push(self.lookup_receipt_lane(&lane, &op).await?);
        }
        Ok(results)
    }

    pub async fn lookup(&mut self, session: &Session, op: &Id) -> Result<Resolution> {
        self.lookup_receipt_lane(&ReceiptLane::Basic(BasicLane::Legacy(session)), op)
            .await
    }

    pub async fn lookup_existing(
        &mut self,
        session: &ExistingBasicSession,
        op: &Id,
    ) -> Result<Resolution> {
        self.lookup_receipt_lane(&ReceiptLane::Basic(BasicLane::Existing(session)), op)
            .await
    }

    pub async fn receipt_status(
        &mut self,
        session: &ReceiptSession,
        op: &Id,
    ) -> Result<Resolution> {
        self.lookup_receipt_lane(&ReceiptLane::Receipt(session), op)
            .await
    }

    async fn lookup_receipt_lane(&mut self, lane: &ReceiptLane<'_>, op: &Id) -> Result<Resolution> {
        self.validate_receipt_lane(lane)?;
        let (selected, _, _) = self.original(op)?;
        let lease = self.hold_container(&selected.intent.container_id)?;
        let (original, _, prior_result) = self.original(op)?;
        if original != selected {
            return Err(invalid("the selected original record changed"));
        }
        // Readback is independent of body availability. No POST can follow from this method.
        let recheck = || {
            self.validate_receipt_lane(lane)?;
            if self.original(op)?.0 != original {
                return Err(invalid("the selected original changed before receipt GET"));
            }
            held_lease_at(&self.root, self.db_id, &lease)
        };
        let reply = match prior_result {
            Some(sequence) => Lookup::Confirmed { sequence },
            None => match lane {
                ReceiptLane::Basic(basic) => {
                    basic
                        .requests(&recheck)
                        .lookup(&original.intent, || Ok(()))
                        .await?
                }
                ReceiptLane::Receipt(session) => session.lookup(&original.intent, recheck).await?,
            },
        };
        if let ReceiptLane::Basic(basic) = lane {
            self.validate_basic_lane(*basic)?;
        }
        self.settle(&original, reply, &lease)
    }

    pub async fn submit(
        &mut self,
        session: &Session,
        op: &Id,
        confirmed: bool,
    ) -> Result<Resolution> {
        self.submit_using(BasicLane::Legacy(session), op, confirmed)
            .await
    }

    pub async fn submit_existing(
        &mut self,
        session: &ExistingBasicSession,
        op: &Id,
        confirmed: bool,
    ) -> Result<Resolution> {
        self.submit_using(BasicLane::Existing(session), op, confirmed)
            .await
    }

    async fn submit_using(
        &mut self,
        lane: BasicLane<'_>,
        op: &Id,
        confirmed: bool,
    ) -> Result<Resolution> {
        if !confirmed {
            return Err(invalid("explicitly confirm this exact original SaveNode"));
        }
        let before_wire = || self.validate_basic_lane(lane);
        before_wire()?;
        let session = lane.requests(&before_wire);
        let (initial, _, _) = self.original(op)?;
        let lease = self.hold_container(&initial.intent.container_id)?;
        let (original, phase, prior) = self.original(op)?;
        if original != initial {
            return Err(invalid("the selected original record changed"));
        }
        if let Some(sequence) = prior {
            return self.settle(&original, Lookup::Confirmed { sequence }, &lease);
        }
        if !can_arm_original(&phase) {
            return Err(invalid(
                "this original has a saved conflict; check its receipt without submitting",
            ));
        }
        let receipt = session
            .lookup(&original.intent, || {
                if self.original(op)?.0 != original {
                    return Err(invalid(
                        "the selected original changed before submit preflight",
                    ));
                }
                held_lease_at(&self.root, self.db_id, &lease)
            })
            .await?;
        self.validate_basic_lane(lane)?;
        if receipt != Lookup::Absent {
            return self.settle(&original, receipt, &lease); // Not permission to send.
        }
        held_lease_at(&self.root, self.db_id, &lease)?;
        if db_basis(&self.db, &original.path)? != original.prior {
            return Err(invalid(
                "original mapping no longer agrees; only look up the prior operation",
            ));
        }
        let source = self.root.source(Path::new(&payload(op)))?;
        if source.before != original.sealed || source.scan(sink())? != original.intent.body {
            return Err(invalid(
                "the exact original sealed body is unavailable; never recapture for this operation",
            ));
        }
        let file = source.file.try_clone()?;
        self.arm_attempt(&original, &source, &lease)?;
        let reply = session
            .save_exact(&original.intent, file, || {
                // H2 readiness may suspend; recheck all custody at the actual wire boundary.
                self.check_attempt(&original, &source, &lease)
            })
            .await?;
        self.validate_basic_lane(lane)?;
        self.settle(&original, reply, &lease)
    }

    fn arm_attempt(&self, original: &Original, source: &Source, lease: &Lease) -> Result<()> {
        source.check()?;
        let (retained, phase, _) = self.original(&original.intent.operation_id)?;
        if retained != *original
            || !can_arm_original(&phase)
            || db_basis(&self.db, &original.path)? != original.prior
        {
            return Err(invalid(
                "authorization, conflict state or original basis changed before arming",
            ));
        }
        held_lease_at(&self.root, self.db_id, lease)?;
        if self.db.execute(
            "UPDATE original SET phase='attempted' WHERE op=?1 AND record=?2 AND phase=?3",
            params![
                original.intent.operation_id.as_str(),
                serde_json::to_string(original)?,
                phase
            ],
        )? != 1
        {
            return Err(invalid("the exact original no longer permits sending"));
        }
        self.root.sync()?;
        held_lease_at(&self.root, self.db_id, lease)
    }

    fn check_attempt(&self, original: &Original, source: &Source, lease: &Lease) -> Result<()> {
        source.check()?;
        let (again, phase, _) = self.original(&original.intent.operation_id)?;
        if again != *original
            || phase != "attempted"
            || db_basis(&self.db, &original.path)? != original.prior
        {
            return Err(invalid(
                "authorization or original basis changed before POST",
            ));
        }
        held_lease_at(&self.root, self.db_id, lease)
    }

    fn settle(
        &mut self,
        original: &Original,
        mut status: Lookup,
        lease: &Lease,
    ) -> Result<Resolution> {
        let root = &self.root;
        let db_id = self.db_id;
        let recheck = || held_lease_at(root, db_id, lease);
        recheck()?;
        let op = &original.intent.operation_id;
        let (selected, phase, prior) = self.original(op)?;
        if &selected != original {
            return Err(invalid("original local pending changed during network"));
        }
        if let Some(sequence) = prior {
            status = Lookup::Confirmed { sequence };
        }
        let status = preserve_original_conflict(&phase, status);
        if let Lookup::Confirmed { sequence } = status {
            if prior.is_none() {
                let tx = self
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let (now, receipt): (String, Option<String>) = tx.query_row(
                    "SELECT record, receipt FROM original WHERE op=?1",
                    [op.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                if serde_json::from_str::<Original>(&now)? != *original {
                    return Err(invalid("original changed before settlement"));
                }
                if let Some(prior) = receipt {
                    if prior.parse::<u64>()? != sequence {
                        return Err(invalid("contradictory original receipt"));
                    }
                } else {
                    if db_basis(&tx, &original.path)? != original.prior {
                        return Err(invalid(
                            "the working mapping changed; leave both original pending and local files untouched",
                        ));
                    }
                    let basis = Basis {
                        container: original.intent.container_id.clone(),
                        branch: Branch::parse(original.intent.target.branch())?,
                        node: original.intent.node_id.clone(),
                        seen_sequence: sequence,
                        body: original.intent.body.clone(),
                    };
                    recheck()?;
                    tx.execute("INSERT INTO basis VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET record=excluded.record",
                               params![&original.path, serde_json::to_string(&basis)?])?;
                    recheck()?;
                    tx.execute(
                        "UPDATE original SET phase='confirmed',receipt=?2 WHERE op=?1",
                        params![op.as_str(), sequence.to_string()],
                    )?;
                }
                recheck()?;
                tx.commit()?;
                root.sync()?;
            }
            let cleaned = self.cleanup(original, lease)?;
            return Ok(Resolution {
                operation: op.clone(),
                status,
                local_cleanup_pending: !cleaned,
            });
        }
        // A receipt observation is not the original's durable ability to send.
        // Keep evidence that an original was never armed, could have been sent,
        // or already conflicts. A missing receipt cannot revoke any of that.
        if status == Lookup::Conflict && phase != "conflict" {
            recheck()?;
            if self.db.execute(
                "UPDATE original SET phase='conflict' WHERE op=?1 AND record=?2 AND phase=?3",
                params![op.as_str(), serde_json::to_string(original)?, phase],
            )? != 1
            {
                return Err(invalid(
                    "the exact pending original changed before recording conflict",
                ));
            }
        }
        recheck()?;
        Ok(Resolution {
            operation: op.clone(),
            status,
            local_cleanup_pending: false,
        })
    }

    fn cleanup(&self, original: &Original, lease: &Lease) -> Result<bool> {
        let recheck = || held_lease_at(&self.root, self.db_id, lease);
        recheck()?;
        let op = original.intent.operation_id.as_str();
        let cleaned: bool =
            self.db
                .query_row("SELECT cleaned FROM original WHERE op=?1", [op], |r| {
                    r.get(0)
                })?;
        if cleaned {
            return Ok(true);
        }
        let file = payload(&original.intent.operation_id);
        if self.root.absent(file.as_ref()).is_err()
            && self
                .root
                .remove_verified(&file, &original.intent.body, original.sealed, recheck)
                .is_err()
        {
            recheck()?;
            return Ok(false); // A local cleanup problem never resurrects network authorization.
        }
        self.root.sync()?;
        recheck()?;
        self.db.execute(
            "UPDATE original SET cleaned=1 WHERE op=?1 AND phase='confirmed'",
            [op],
        )?;
        Ok(true)
    }

    pub fn review_bundle(&self, path: &Path) -> Result<bundle::Review> {
        self.validate()?;
        self.normal_path(path)?;
        bundle::Review::read(path)
    }

    pub fn diff_bundle(&self, review: &bundle::Review) -> Result<bundle::Diff> {
        self.validate()?;
        let path = self.normal_path(review.root())?;
        let previous: Option<String> = self
            .db
            .query_row(
                "SELECT record FROM bundle_manifest WHERE path=?1",
                [path],
                |r| r.get(0),
            )
            .optional()?;
        let previous = previous
            .map(|row| serde_json::from_str::<bundle::Manifest>(&row))
            .transpose()?;
        Ok(review.manifest.diff(previous.as_ref()))
    }

    pub fn pack_bundle(
        &mut self,
        review: &bundle::Review,
        destination: &bundle::Destination,
        confirmed: bool,
    ) -> Result<Body> {
        self.validate()?;
        let path = self.normal_path(review.root())?;
        self.normal_path(destination.path())?;
        let body = destination.publish(review, confirmed)?;
        // This is a last locally packaged manifest, NOT a server version/recovery point.
        let saved = self.db.execute(
            "INSERT INTO bundle_manifest VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET record=excluded.record",
            params![path, serde_json::to_string(&review.manifest)?]);
        if saved.is_err() || self.held_database().is_err() {
            return Err(invalid(
                "ZIP already published; not all local diff bookkeeping confirmed",
            ));
        }
        Ok(body)
    }
}

impl PayloadWorkspace {
    pub fn open_local(directory: &Path) -> Result<Self> {
        let (workspace, original_file) = Workspace::connect_with_access(
            Folder::open(directory, true)?,
            None,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let opened = Self(workspace, original_file);
        opened.checked_read(|| Ok(()))?;
        Ok(opened)
    }

    fn checked_read<T>(&self, inspect: impl FnOnce() -> Result<T>) -> Result<T> {
        // Retain the original raw FD for the lifetime of the SQLite connection.
        // Opening and closing another FD on this inode could release POSIX locks.
        inspect_payload_database(&self.0.root, &self.1, || {
            self.0.validate()?;
            inspect()
        })
    }

    /// Select an already-confirmed immutable node, without reading local file bytes.
    pub fn choose_payload_slice(
        &self,
        mapped: &Path,
        window: ByteWindow,
        output: &Path,
    ) -> Result<PayloadSlice> {
        let mapped = self.0.normal_path(mapped)?.to_owned();
        let selected = self.checked_read(|| {
            db_basis(&self.0.db, &mapped)?.ok_or_else(|| PayloadError::MissingBaseline.into())
        })?;
        let target = self.0.normal_path(output)?.to_owned();
        let parent = Folder::open(
            output
                .parent()
                .ok_or_else(|| invalid("slice output needs its original parent"))?,
            false,
        )?;
        let leaf = output
            .file_name()
            .ok_or_else(|| invalid("slice output needs a new ordinary name"))?;
        let selection = PayloadSelection::new(
            selected.container.clone(),
            selected.node.clone(),
            selected.body.clone(),
            window,
        );
        let choice = PayloadSlice {
            owner: Arc::clone(&self.0.read_session_owner),
            mapped,
            selected,
            selection,
            output: target,
            parent,
            leaf: leaf.to_owned(),
        };
        self.check_payload_slice(&choice)?;
        Ok(choice)
    }

    fn check_payload_slice(&self, choice: &PayloadSlice) -> Result<()> {
        if !Arc::ptr_eq(&self.0.read_session_owner, &choice.owner) {
            return Err(invalid(
                "the payload choice has a different original workspace",
            ));
        }
        self.checked_read(|| {
            if db_basis(&self.0.db, &choice.mapped)?.as_ref() != Some(&choice.selected) {
                return Err(invalid(
                    "the original confirmed payload baseline changed or is missing",
                ));
            }
            if choice.output == choice.mapped
                || db_basis(&self.0.db, &choice.output)?.is_some()
                || unresolved_path(&self.0.db, &choice.output)?
            {
                return Err(invalid(
                    "slice output must be a new unmapped ordinary filename",
                ));
            }
            choice.parent.absent(&choice.leaf)
        })
    }

    /// Explicit login for this exact workspace and still-selected immutable slice.
    pub async fn connect_payload(
        &self,
        choice: &PayloadSlice,
        spelling: &str,
        account: &str,
        prompt: impl FnOnce() -> Result<Zeroizing<String>>,
    ) -> Result<PayloadSession> {
        self.check_payload_slice(choice)?;
        PayloadSession::connect_bound(
            &self.0.principal,
            Arc::clone(&self.0.read_session_owner),
            &choice.selection,
            spelling,
            account,
            prompt,
            || self.check_payload_slice(choice),
        )
        .await
    }

    /// Verify one slice into an unnamed inode, then publish once with no overwrite.
    /// This never inserts a workspace mapping or changes any remote state.
    pub async fn save_payload_slice(
        &self,
        session: &PayloadSession,
        choice: &PayloadSlice,
    ) -> Result<Body> {
        let recheck = || {
            session.require_choice(
                &self.0.read_session_owner,
                &self.0.principal,
                &choice.selection,
            )?;
            self.check_payload_slice(choice)
        };
        recheck()?;
        let mut staged = choice.parent.unlinked()?;
        let body = session.read_slice(&mut staged, recheck).await?;
        choice
            .parent
            .publish_guarded(&choice.leaf, &staged, &body, recheck)?;
        Ok(body)
    }
}

fn check_workspace_header(file: &Source, require_rollback: bool) -> Result<()> {
    let mut header = [0_u8; 100];
    file.file.read_exact_at(&mut header, 0)?;
    validate_workspace_header(&header, require_rollback)?;
    file.check()
}

fn validate_workspace_header(header: &[u8; 100], require_rollback: bool) -> Result<()> {
    if &header[..16] != b"SQLite format 3\x00"
        || header[60..64] != (VERSION as u32).to_be_bytes()
        || header[68..72] != (APP as u32).to_be_bytes()
    {
        return Err(invalid(
            "not the independent workspace DB header; leaving it unopened by SQLite",
        ));
    }
    // WAL read/write markers can make even OPEN_READ_ONLY create WAL or SHM.
    if require_rollback && header[18..20] != [1, 1] {
        return Err(invalid(
            "payload reads require the original rollback-mode SQLite file",
        ));
    }
    Ok(())
}

fn inspect_payload_database<T>(
    root: &Folder,
    file: &Source,
    inspect: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let check = || {
        file.check()?;
        if file.file.metadata()?.mode() & 0o777 != 0o600 {
            return Err(invalid("workspace SQLite file is not private 0600"));
        }
        for name in DB_SIDECARS {
            // Do not open, interpret or try to recover any auxiliary object.
            root.absent(OsStr::new(name)).map_err(|_| {
                invalid(
                    "payload reads require an exact SQLite directory without journal, WAL or SHM",
                )
            })?;
        }
        check_workspace_header(file, true)
    };
    check()?;
    let result = inspect();
    check()?;
    result
}

fn can_arm_original(phase: &str) -> bool {
    matches!(phase, "prepared" | "attempted" | "unknown" | "absent")
}

fn preserve_original_conflict(phase: &str, observation: Lookup) -> Lookup {
    if phase == "conflict" && !matches!(observation, Lookup::Confirmed { .. }) {
        Lookup::Conflict
    } else {
        observation
    }
}

fn held_database_at(root: &Folder, db_id: (u64, u64)) -> Result<()> {
    root.check()?;
    let named = root.pin(Path::new(DB))?.metadata()?;
    if !named.is_file()
        || named.uid() != native::ordinary_uid()?
        || named.mode() & 0o777 != 0o600
        || named.nlink() != 1
        || (named.dev(), named.ino()) != db_id
    {
        return Err(invalid(
            "the original private SQLite identity no longer matches",
        ));
    }
    Ok(())
}

fn held_lease_at(root: &Folder, db_id: (u64, u64), lease: &Lease) -> Result<()> {
    held_database_at(root, db_id)?;
    lease.check()
}

fn require_profile(db: &Connection, principal: &Principal) -> Result<()> {
    if schema_version(db)? != (APP, VERSION) {
        return Err(invalid("not independent ZXS Rust client state"));
    }
    let profile: String =
        db.query_row("SELECT record FROM profile WHERE id=1", [], |r| r.get(0))?;
    if &serde_json::from_str::<Principal>(&profile)? != principal {
        return Err(invalid(
            "workspace state needs the original server trust and account",
        ));
    }
    Ok(())
}

fn schema_version(db: &Connection) -> Result<(i64, i64)> {
    Ok((
        db.pragma_query_value(None, "application_id", |r| r.get(0))?,
        db.pragma_query_value(None, "user_version", |r| r.get(0))?,
    ))
}

fn unresolved_selection(db: &Connection, container: &Id, path: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM original WHERE (container=?1 OR path=?2) AND phase!='confirmed')",
        params![container.as_str(), path], |r| r.get(0))?)
}

fn unresolved_path(db: &Connection, path: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM original WHERE path=?1 AND phase!='confirmed')",
        [path],
        |r| r.get(0),
    )?)
}

fn db_basis(db: &Connection, path: &str) -> Result<Option<Basis>> {
    let row: Option<String> = db
        .query_row("SELECT record FROM basis WHERE path=?1", [path], |r| {
            r.get(0)
        })
        .optional()?;
    row.map(|value| serde_json::from_str::<Basis>(&value).map_err(Into::into))
        .transpose()
}

fn payload(op: &Id) -> String {
    format!("pending-{op}.body")
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
