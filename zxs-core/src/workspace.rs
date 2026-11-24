//! Local mappings, frozen unconfirmed payloads, and verified R3 receipt reconciliation.
use crate::{
    Result, bundle, invalid,
    native::{self, Folder, Lease, Source, Stamp},
    r3::{Body, Branch, DisplayName, Id, Intent, Lookup, Target},
    session::{Principal, ReceiptSession, Session},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, Permissions},
    io::sink,
    os::unix::fs::{FileExt, MetadataExt, PermissionsExt},
    path::Path,
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

const DB: &str = "workspace.sqlite";
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
    receipt_owner: Arc<()>,
}

enum ReceiptLane<'a> {
    Basic(&'a Session),
    Receipt(&'a ReceiptSession),
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
        let db_file = root.source(Path::new(DB))?;
        if db_file.file.metadata()?.mode() & 0o777 != 0o600 {
            return Err(invalid("workspace SQLite file is not private 0600"));
        }
        if enrolled.is_some() {
            if db_file.before.length != 0 {
                return Err(invalid("refusing to initialize a nonempty SQLite file"));
            }
        } else {
            let mut header = [0_u8; 100];
            db_file.file.read_exact_at(&mut header, 0)?;
            if &header[..16] != b"SQLite format 3\x00"
                || header[60..64] != (VERSION as u32).to_be_bytes()
                || header[68..72] != (APP as u32).to_be_bytes()
            {
                return Err(invalid(
                    "not the independent workspace DB header; leaving it unopened by SQLite",
                ));
            }
            db_file.check()?;
        }
        let db_id = (db_file.before.dev, db_file.before.ino);
        let db = Connection::open_with_flags(
            root.path().join(DB),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "trusted_schema", "OFF")?;
        db.pragma_update(None, "synchronous", "EXTRA")?;
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
        let opened = Self {
            root,
            db,
            db_id,
            principal,
            receipt_owner: Arc::new(()),
        };
        opened.held_database()?;
        Ok(opened)
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
        if schema_version(&self.db)? != (APP, VERSION) {
            return Err(invalid("not independent ZXS Rust client state"));
        }
        let profile: String =
            self.db
                .query_row("SELECT record FROM profile WHERE id=1", [], |r| r.get(0))?;
        if serde_json::from_str::<Principal>(&profile)? != self.principal {
            return Err(invalid(
                "workspace state needs the original server trust and account",
            ));
        }
        self.held_database()
    }

    fn same_principal(&self, session: &Session) -> Result<()> {
        self.validate()?;
        if session.principal() != &self.principal {
            return Err(invalid("original account/trust changed"));
        }
        Ok(())
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
            Arc::clone(&self.receipt_owner),
            spelling,
            account,
            prompt,
            || self.validate(),
        )
        .await
    }

    fn validate_receipt_lane(&self, lane: &ReceiptLane<'_>) -> Result<()> {
        match lane {
            ReceiptLane::Basic(session) => self.same_principal(session),
            ReceiptLane::Receipt(session) => {
                self.validate()?;
                if !session.belongs_to(&self.receipt_owner, &self.principal) {
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

    pub async fn checkout(
        &mut self,
        session: &Session,
        container: &Id,
        branch: &Branch,
        file: &Path,
    ) -> Result<Basis> {
        self.same_principal(session)?;
        let path = self.normal_path(file)?.to_owned();
        if self.basis(file)?.is_some() || unresolved_path(&self.db, &path)? {
            return Err(invalid(
                "local file already owns a workspace binding or an unresolved original",
            ));
        }
        let parent = Folder::open(
            file.parent()
                .ok_or_else(|| invalid("checkout needs an original parent"))?,
            false,
        )?;
        let leaf = file
            .file_name()
            .ok_or_else(|| invalid("checkout needs a new ordinary leaf"))?;
        parent.absent(leaf)?;
        let (detail, node) = session.selected_head(container, branch).await?;
        let mut staged = parent.unlinked()?;
        session.download(container, &node, &mut staged).await?;
        self.held_database()?;
        let basis = Basis {
            container: container.clone(),
            branch: branch.clone(),
            node: node.node_id.clone(),
            seen_sequence: detail.sequence,
            body: node.body(),
        };
        // Checkout has no network mutation or pending operation. Its local write
        // is guarded by the held DB/path ownership transaction and held destination.
        let root = &self.root;
        let db_id = self.db_id;
        let recheck = || held_database_at(root, db_id);
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if db_basis(&tx, &path)?.is_some() || unresolved_path(&tx, &path)? {
            return Err(invalid(
                "a binding or unresolved original acquired the destination during download",
            ));
        }
        parent.publish_guarded(leaf, &staged, &basis.body, recheck)?;
        let saved = (|| -> Result<()> {
            recheck()?;
            parent.check()?;
            tx.execute(
                "INSERT INTO basis VALUES(?1,?2)",
                params![path, serde_json::to_string(&basis)?],
            )?;
            recheck()?;
            parent.check()?;
            tx.commit()?;
            Ok(())
        })();
        if saved.is_err() || self.held_database().is_err() {
            return Err(invalid(
                "download may already be published, but local binding not confirmed; do not overwrite",
            ));
        }
        Ok(basis)
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
        self.reconcile_receipt_lane(ReceiptLane::Basic(session))
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
        self.lookup_receipt_lane(&ReceiptLane::Basic(session), op)
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
        let reply = match prior_result {
            Some(sequence) => Lookup::Confirmed { sequence },
            None => match lane {
                ReceiptLane::Basic(session) => session.lookup(&original.intent).await,
                ReceiptLane::Receipt(session) => {
                    session
                        .lookup(&original.intent, || {
                            self.validate_receipt_lane(lane)?;
                            if self.original(op)?.0 != original {
                                return Err(invalid(
                                    "the selected original changed before receipt GET",
                                ));
                            }
                            held_lease_at(&self.root, self.db_id, &lease)
                        })
                        .await?
                }
            },
        };
        self.settle(&original, reply, &lease)
    }

    pub async fn submit(
        &mut self,
        session: &Session,
        op: &Id,
        confirmed: bool,
    ) -> Result<Resolution> {
        if !confirmed {
            return Err(invalid("explicitly confirm this exact original SaveNode"));
        }
        self.same_principal(session)?;
        let (initial, _, _) = self.original(op)?;
        let lease = self.hold_container(&initial.intent.container_id)?;
        let (original, _, prior) = self.original(op)?;
        if original != initial {
            return Err(invalid("the selected original record changed"));
        }
        if let Some(sequence) = prior {
            return self.settle(&original, Lookup::Confirmed { sequence }, &lease);
        }
        let receipt = session.lookup(&original.intent).await;
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
                self.same_principal(session)?;
                self.check_attempt(&original, &source, &lease)
            })
            .await?;
        self.settle(&original, reply, &lease)
    }

    fn arm_attempt(&self, original: &Original, source: &Source, lease: &Lease) -> Result<()> {
        source.check()?;
        let (retained, _, _) = self.original(&original.intent.operation_id)?;
        if retained != *original || db_basis(&self.db, &original.path)? != original.prior {
            return Err(invalid(
                "authorization or original basis changed before arming",
            ));
        }
        held_lease_at(&self.root, self.db_id, lease)?;
        if self.db.execute(
            "UPDATE original SET phase='attempted' WHERE op=?1 AND record=?2 AND phase!='confirmed'",
            params![original.intent.operation_id.as_str(), serde_json::to_string(original)?],
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
        let (selected, _, prior) = self.original(op)?;
        if &selected != original {
            return Err(invalid("original local pending changed during network"));
        }
        if let Some(sequence) = prior {
            status = Lookup::Confirmed { sequence };
        }
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
        let phase = match status {
            Lookup::Absent => "absent",
            Lookup::Conflict => "conflict",
            _ => "unknown",
        };
        recheck()?;
        if self.db.execute(
            "UPDATE original SET phase=?2 WHERE op=?1 AND record=?3 AND phase!='confirmed'",
            params![op.as_str(), phase, serde_json::to_string(original)?],
        )? != 1
        {
            return Err(invalid(
                "the exact pending original changed before settlement",
            ));
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
