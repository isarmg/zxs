//! User-initiated protected GETs. A read cannot submit a write. An account,
//! root, selected occurrence, entered number or generation change fences its reply.
use super::*;
use crate::ui::accept_read_response;
use storage_contract::snapshot_worktree::outcome::WorktreeCommitOutcome;
use web_interface::tasks::spawn_local;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Reader {
    scope: Scope, actor: Principal, pub(super) status: WorktreeStatus, operation: OperationId,
}

impl Reader {
    pub(super) fn capture(s: &State) -> Option<Self> {
        if !available(s) { return None; }
        let obs = s.files.directory_snapshot.worktree.observed(s)?;
        Some(Self { scope: obs.scope.clone(), actor: s.principal.clone()?,
            status: obs.status.clone(), operation: form(s)?.operation.parse().ok()? })
    }

    pub(super) fn admit(&self, value: json_codec::Value) -> Result<Proof, api::Error> {
        let original = wire::worktree_commit_receipt(value)?;
        let WorktreeCommitOutcome::Complete { snapshot } = original.result else {
            return Err(wire::invalid("the original worktree commit is still pending"));
        };
        if original.repository_id != self.scope.repository || original.request.operation_id != self.operation
            || original.request.target != self.status.target || original.request.expected != self.status.revision
            || snapshot.repository_id != self.scope.snapshot_repository || snapshot.node_id == self.status.original.node_id
        {
            return Err(wire::invalid("the original commit is not for this opened instance and revision"));
        }
        Ok(Proof { scope: self.scope.clone(), actor: self.actor.clone(), target: self.status.target,
            source: self.status.original, expected: self.status.revision, operation: self.operation, committed: snapshot })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Query { scope: Scope, actor: Principal, operation: OperationId }

impl Query {
    pub(super) fn capture(s: &State) -> Option<Self> {
        if !s.management.storage.readable() { return None; }
        Some(Self { scope: Scope::capture(s)?, actor: s.principal.clone()?, operation: form(s)?.lookup.parse().ok()? })
    }

    pub(super) fn admit(&self, value: json_codec::Value) -> Result<MountChangeReceipt, api::Error> {
        let r = wire::mount::receipt(value)?;
        if r.request.operation_id != self.operation
            || matches!(&r.request.action, MountAction::Hide { committed, .. }
                if committed.repository_id != self.scope.snapshot_repository)
        {
            return Err(wire::invalid("the original mount receipt differs from the requested scope"));
        }
        Ok(r)
    }
}

pub(super) fn observed(s: &State) -> Option<&MountChangeReceipt> {
    let (q, receipt) = s.files.directory_snapshot.worktree.mount.seen.as_ref()?;
    (Query::capture(s).as_ref() == Some(q)).then_some(receipt)
}

#[derive(Clone, Copy)]
pub(super) enum Kind { Commit, Mount }

#[derive(Clone, PartialEq, Eq)]
enum Read { Commit(Reader), Mount(Query) }
enum Admitted { Commit(Proof), Mount(Query, MountChangeReceipt) }
impl Kind {
    fn capture(self, s: &State) -> Option<Read> {
        match self {
            Self::Commit => Reader::capture(s).map(Read::Commit),
            Self::Mount => Query::capture(s).map(Read::Mount),
        }
    }
    pub(super) fn start(self, s: RwSignal<State>) {
        let Some(read) = s.with_untracked(|s| self.capture(s)) else { return; };
        let cancel = match api::ReadCancellation::new() {
            Ok(c) => c,
            Err(e) => { s.update(|s| s.files.directory_snapshot.error = Some(e)); return; }
        };
        let Some(serial) = s.try_update(|s| {
            let m = &mut s.files.directory_snapshot.worktree.mount;
            m.cancel();
            match self { Self::Commit => m.proof = None, Self::Mount => m.seen = None }
            m.reading = true;
            m.cancel = Some(cancel.clone());
            m.generation
        }) else { return; };
        spawn_local(async move {
            let (epoch, url) = match &read {
                Read::Commit(r) => (r.scope.epoch, format!("/api/repositories/{}/directory-worktrees/commits/{}", r.scope.repository, r.operation)),
                Read::Mount(q) => (q.scope.epoch, format!("/api/repositories/{}/directory-worktrees/mount-changes/{}", q.scope.repository, q.operation)),
            };
            let result = api::cancellable_get(&url, cancel).await.and_then(|v| match &read {
                Read::Commit(r) => r.admit(v).map(Admitted::Commit),
                Read::Mount(q) => q.admit(v).map(|r| Admitted::Mount(q.clone(), r)),
            });
            if !accept_read_response(s, epoch, &result) { return; }
            let _ = s.try_update(|s| {
                let current = self.capture(s).as_ref() == Some(&read);
                let m = &mut s.files.directory_snapshot.worktree.mount;
                if m.generation != serial { return; }
                m.reading = false;
                m.cancel = None;
                if !current { return; }
                match result {
                    Ok(result) => {
                        match result { Admitted::Commit(p) => m.proof = Some(p), Admitted::Mount(q, r) => m.seen = Some((q, r)) }
                        s.files.directory_snapshot.error = None;
                    }
                    Err(e) => s.files.directory_snapshot.error = Some(e),
                }
            });
        });
    }
}
