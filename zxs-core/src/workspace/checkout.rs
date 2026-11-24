//! Held before confirmation, then one exact read-only remote checkout.
use super::{
    BasicLane, Basis, Workspace, db_basis, held_database_at, require_profile, unresolved_path,
};
use crate::{
    Result, invalid,
    native::{Folder, VerifiedOutput},
    r3::{Branch, Detail, Id, Node},
    session::{ExistingBasicSession, Session},
};
use rusqlite::{Connection, TransactionBehavior, params};
use std::{ffi::OsString, fs::File, path::Path, sync::Arc};

/// One opaque workspace/session-bound choice. It holds the actual selected
/// output directory, but selection creates no file and no workspace mapping.
pub struct CheckoutChoice {
    session: Arc<()>,
    selected: OriginalHead,
    target: Destination,
}

impl CheckoutChoice {
    pub fn basis(&self) -> &Basis {
        &self.selected.basis
    }

    pub fn output(&self) -> &Path {
        Path::new(&self.target.path)
    }

    fn require_session(&self, session: &ExistingBasicSession) -> Result<()> {
        require_same_session(&self.session, session.checkout_session())
    }
}

fn require_same_session(selected: &Arc<()>, actual: &Arc<()>) -> Result<()> {
    if !Arc::ptr_eq(selected, actual) {
        return Err(invalid(
            "the checkout requires the same originally selected Basic session",
        ));
    }
    Ok(())
}

struct OriginalHead {
    basis: Basis,
    node: Node,
}

impl OriginalHead {
    fn new(container: &Id, branch: &Branch, detail: &Detail, node: Node) -> Self {
        Self {
            basis: Basis {
                container: container.clone(),
                branch: branch.clone(),
                node: node.node_id.clone(),
                seen_sequence: detail.sequence,
                body: node.body(),
            },
            node,
        }
    }

    fn require_detail(&self, detail: &Detail) -> Result<()> {
        if detail.container_id != self.basis.container
            || detail.sequence != self.basis.seen_sequence
            || !detail
                .heads
                .iter()
                .any(|head| head.branch == self.basis.branch && head.head == self.basis.node)
        {
            return Err(invalid(
                "the original checkout head or revision changed; choose it again",
            ));
        }
        Ok(())
    }

    fn require_node(&self, detail: &Detail, node: &Node) -> Result<()> {
        self.require_detail(detail)?;
        if &self.node != node {
            return Err(invalid(
                "the original immutable checkout node no longer matches",
            ));
        }
        Ok(())
    }
}

struct Destination {
    path: String,
    parent: Folder,
    leaf: OsString,
}

impl Destination {
    fn require_vacant(&self, db: &Connection) -> Result<()> {
        if db_basis(db, &self.path)?.is_some() || unresolved_path(db, &self.path)? {
            return Err(invalid(
                "local file already owns a workspace binding or an unresolved original",
            ));
        }
        self.parent.absent(&self.leaf)
    }

    fn verify<'a>(&'a self, staged: &'a File, basis: &Basis) -> Result<StagedCheckout<'a>> {
        let output = self
            .parent
            .verify_unnamed(&self.leaf, staged, &basis.body)?;
        Ok(StagedCheckout {
            target: self,
            output,
        })
    }
}

// Keep the same target and verified native name paired through the local cut.
struct StagedCheckout<'a> {
    target: &'a Destination,
    output: VerifiedOutput<'a>,
}

impl Workspace {
    /// Hold the exact new output name and resolve only the node already
    /// selected by the caller. The branch must be in the first R3 detail page.
    /// No file, mapping, new login or remote mutation is created.
    pub async fn choose_checkout(
        &self,
        session: &ExistingBasicSession,
        container: &Id,
        branch: &Branch,
        expected_node: &Id,
        output: &Path,
    ) -> Result<CheckoutChoice> {
        self.validate_basic_lane(BasicLane::Existing(session))?;
        let target = self.checkout_destination(output)?;
        let guard = || {
            self.validate_basic_lane(BasicLane::Existing(session))?;
            target.require_vacant(&self.db)
        };
        let (detail, node) = session
            .requests(&guard)
            .selected_head_at(container, branch, Some(expected_node), None)
            .await?;
        guard()?;
        let selected = OriginalHead::new(container, branch, &detail, node);
        Ok(CheckoutChoice {
            session: Arc::clone(session.checkout_session()),
            selected,
            target,
        })
    }

    /// Consume the same confirmed choice. An observed head/revision change
    /// requires another choice. The final remote read is not a reservation;
    /// after it, the only publishable bytes still belong to the original node.
    pub async fn checkout_selected(
        &mut self,
        session: &ExistingBasicSession,
        choice: CheckoutChoice,
        confirmed: bool,
    ) -> Result<Basis> {
        if !confirmed {
            return Err(invalid(
                "confirm the exact checkout before reading or publishing it",
            ));
        }
        let guard = || {
            choice.require_session(session)?;
            self.validate_basic_lane(BasicLane::Existing(session))?;
            choice.target.require_vacant(&self.db)
        };
        guard()?;
        let basis = choice.basis();
        let request = session.requests(&guard);
        let (detail, node) = request
            .selected_head_at(
                &basis.container,
                &basis.branch,
                Some(&basis.node),
                Some(basis.seen_sequence),
            )
            .await?;
        choice.selected.require_node(&detail, &node)?;
        guard()?;
        let mut staged = choice.target.parent.unlinked()?;
        request
            .download(&basis.container, &choice.selected.node, &mut staged)
            .await?;
        // Finish the potentially long local readback before the final read
        // of the remote choice. No network await after opening the SQLite cut.
        let ready = choice.target.verify(&staged, basis)?;
        let final_detail = request
            .detail_at(&basis.container, Some(basis.seen_sequence))
            .await?;
        choice.selected.require_detail(&final_detail)?;
        guard()?;
        let basis = basis.clone();
        self.publish_checkout(ready, basis)
    }

    pub async fn checkout(
        &mut self,
        session: &Session,
        container: &Id,
        branch: &Branch,
        file: &Path,
    ) -> Result<Basis> {
        self.checkout_using(BasicLane::Legacy(session), container, branch, file)
            .await
    }

    pub async fn checkout_existing(
        &mut self,
        session: &ExistingBasicSession,
        container: &Id,
        branch: &Branch,
        file: &Path,
    ) -> Result<Basis> {
        self.checkout_using(BasicLane::Existing(session), container, branch, file)
            .await
    }

    async fn checkout_using(
        &mut self,
        lane: BasicLane<'_>,
        container: &Id,
        branch: &Branch,
        file: &Path,
    ) -> Result<Basis> {
        let before_wire = || self.validate_basic_lane(lane);
        before_wire()?;
        let session = lane.requests(&before_wire);
        let target = self.checkout_destination(file)?;
        let (detail, node) = session.selected_head(container, branch).await?;
        let mut staged = target.parent.unlinked()?;
        session.download(container, &node, &mut staged).await?;
        self.validate_basic_lane(lane)?;
        let basis = OriginalHead::new(container, branch, &detail, node).basis;
        let ready = target.verify(&staged, &basis)?;
        self.publish_checkout(ready, basis)
    }

    fn checkout_destination(&self, file: &Path) -> Result<Destination> {
        let path = self.normal_path(file)?.to_owned();
        let parent = Folder::open(
            file.parent()
                .ok_or_else(|| invalid("checkout needs an original parent"))?,
            false,
        )?;
        let leaf = file
            .file_name()
            .ok_or_else(|| invalid("checkout needs a new ordinary leaf"))?
            .to_owned();
        let target = Destination { path, parent, leaf };
        target.require_vacant(&self.db)?;
        Ok(target)
    }

    fn publish_checkout(&mut self, ready: StagedCheckout<'_>, basis: Basis) -> Result<Basis> {
        // Local-only namespace/basis transaction. One exact no-replace link;
        // after a possible effect neither retry nor try to delete the new name.
        let StagedCheckout { target, output } = ready;
        let root = &self.root;
        let db_id = self.db_id;
        let profile = &self.principal;
        let database = |db: &Connection| -> Result<()> {
            held_database_at(root, db_id)?;
            require_profile(db, profile)?;
            held_database_at(root, db_id)
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before_link = || {
            database(&tx)?;
            target.require_vacant(&tx)
        };
        before_link()?;
        let named = output.publish(before_link)?;
        let recheck = || -> Result<()> {
            database(&tx)?;
            if output.require_named()? != named {
                return Err(invalid(
                    "the published original changed before local binding",
                ));
            }
            Ok(())
        };
        let saved = (|| -> Result<()> {
            recheck()?;
            tx.execute(
                "INSERT INTO basis VALUES(?1,?2)",
                params![&target.path, serde_json::to_string(&basis)?],
            )?;
            recheck()?;
            Ok(())
        })();
        if saved.is_err() || tx.commit().is_err() || self.held_database().is_err() {
            return Err(invalid(
                "download may already be published, but local binding not confirmed; do not overwrite",
            ));
        }
        Ok(basis)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/checkout_choice_cases.rs"]
mod tests;
