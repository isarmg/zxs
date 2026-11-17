# Exact historical worktree commits

## Decision

The selected immutable source never changes. Under the existing exclusive Engine
writer, commit observes that source branch's admitted directory HEAD. A matching
HEAD continues that branch; an older source allocates an independent branch and
node. The local ledger's existing Continue/Fork transitions carry this decision.

The new branch display name is the instance's already allocated mount name. It
is admitted as BranchName and checked for collision, not used as identity. The
existing actual worktree allocator is not duplicated. A 128-byte branch-name
limit or an existing same-name FILE/directory branch rejects rather than
truncating, replacing or silently choosing another name.

The selected FILE source is independent of directory HEAD. If its exact original
branch/node remains current, the existing FILE writer continues it. Otherwise,
its real file.branch.create operation forks the exact original node/version;
the same existing workspace/version writer consumes the saved edit's reserved
version. The old FILE branch and its selected ordinary view are not redirected.

## Why an original and a ready record

A new FILE branch must exist before the existing workspace writer can create its
workspace and capture the exact workspace request digest. Publishing that branch
before recording the user's original commit would leave an unowned side effect
and let a retry allocate again. Therefore:

1. `snapshot.worktree.historical-reserve` records the original CommitWorktree,
   precise source, revision, checkpoint, saved leaf/entity, observed source HEAD,
   directory destination and all child operation identities.
2. The existing FILE branch constructor runs only when needed, with its recorded
   operation and exact source. Its permanent receipt supplies the actual FILE
   branch/base identities; a name never supplies them.
3. `snapshot.worktree.historical-ready` independently binds that exact original
   commitment to the real workspace request digest and FILE branch/base.
4. The existing FILE publication, held-source MOVE/COPY node constructor, node
   publication and independent completion finish the same original operation.

The existing `ReserveCommit`, `CommitReservation`, node and completion encodings
are unchanged. New variants are explicit, with no defaults, reinterpretation or
old-disk migration. Original outcome GET still returns the unchanged public
CommitWorktree and Pending/Complete result. A downstream failure after the new
original was recorded is reported as a published operation, not a fresh refusal.

## Pending and recovery

Before ready, or after ready but before the independent node publication, an
exact retry remains Pending. It does not repeat a FILE branch, discover a local
workspace, allocate another branch, recapture HEAD or adopt material. Both source
and destination branches and the exact instance stay occupied. After independent
node publication, the existing two-frame ledger/SYSTEM tail recovery uses the
recorded routing; completed retries only reread their exact result.

This is the existing single edited occurrence and Initial/root storage gate. It
does not implement multi-file or deleted-container reconciliation, arbitrary
layout migration, a second writer, new SMB transport, or incremental body saves.
The existing FILE workspace adapter still reads/stages the complete edit body;
this change does not claim an all-streaming or no-full-read implementation.

## Verification scope

Dedicated Engine fixtures cover source/FILE HEAD advancement, exact fork parents,
actual inode MOVE/COPY, continuation from a newly published fork, cold exact
receipts and zero-write retries. A real loopback HTTP fixture submits two saved
instances concurrently and expects one continuation and one historical fork.
One existing failpoint boundary after the new original verifies Pending without
FILE branch creation. Execution results belong to the root validation log; this
ADR is not a claim that the tests or external Samba/browser ran.
