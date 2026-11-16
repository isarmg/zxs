# Source-prefix address continuity before disk-name publication

Source baseline: `7e7f760469550da90872e2b8de6f10b34dd5df32`.
This change preserves an existing structural proof in its real hot/cold consumers.
It neither implements a disk-name mutation nor changes any persisted bytes.

## Current code, not a new authority

`fresh_layout/containers/prefixes.rs::collect` already verifies the complete
FILE and namespace-directory source-prefix references together. A repeated
`(target DiskId, source DiskId)` must have the same direct parent source, original
preparation, prefix inode and five area inodes. Previously its second parent map
was discarded on return. The resulting bare record map could not carry that
parent check into later live reuse and cold feedback.

The same inventory now retains a private `PrefixAddress { target, source, parent }`
with the unchanged `ContainerPrefix`. It is consumed at the existing three edges:

- `containers::validate_state`: the same entity/preparation uniqueness checks.
- `prefixes::plan_source`: a known source prefix is reused only under its exact
  original parent; original registered-source and held-root checks remain first.
- `containers::verify_transition`: FILE and independent directory feedback use
  the same parent-bound chain check. Existing replicas are not retargeted.

No new caller supplies a path, Config, free inode, or permission flag. The type
contains no name and cannot open a root. `PreparedBackupSourcePath` still binds the
actual registered namespace, full UUID/registration chain, Config occurrence and
held roots. `ordinary_preparation::prefix::bind_chain` still opens only its exact
named prefix or the selected decision's original EXCL preparation. It verifies
all five areas and keeps its original NOREPLACE/durability/recovery gates.

This is not a persistent catalog or a terminal-name lookup product. A structural
inventory, including a valid five-area tuple, never grants relocation or lifecycle
permission. Names remain taken only from the already admitted current Config.

## Why the existing FILE rename proof cannot authorize disk names

The ordinary FILE rename's permanent path fact binds an exact old/new
`ContainerNamespace` for one FileId. It can relocate that same container's
historical reads after actual Published/FINALIZE evidence. A source prefix is a
different owner: it contains many containers, independent directories and local
version/publication proofs. Its durable record binds UUID/preparation/entities,
not a new disk Name or a Config successor.

`GenesisIoAuthority::from_config` still includes `DiskConfig.name` exhaustively.
`permits_genesis_recovery_projection`, `same_disk`, portable effective Config and
held-root admission must not accept hand-edited names. Likewise, a historical
source-local proof cannot be found by re-deriving its old physical spelling once
a real later source-area rename has moved it. Current Config text is not evidence
of that later rename. This patch changes none of those gates.

## Next enabled transaction must close these concrete boundaries together

1. The real actor/request/idempotency path must retain the original receipt before
   checking a new write. A single request changes one existing disk's Name only;
   full UUID, slot/binding, topology, RootBinding, five areas, limits and all other
   fields carry exactly. Duplicate Name.key and actual component budgets are checked.
2. The shared inventory yields all actual affected source addresses. For A -> A2,
   move B/A and C/B/A within their same parents. For B -> B2, move only C/B;
   its nested A moves with it. Every referenced child and all five areas retain
   the original inode/preparation. Missing required roots reject before decision.
3. A strict permanent release under the original authenticated global decision
   binds old/new logical names, original Config/control epoch and the complete
   ordered physical obligation. It cannot be a receipt-kind whitelist, a raw
   replacement Config or a rewrite of original Genesis/operation bytes.
4. Before source-local proof reads, cold discovery from fixed disk-global evidence
   must authenticate the exact terminal/pending source-name mapping. Full replay
   still verifies original snapshots and business/payload facts. Logical historic
   Config and terminal physical location remain separate. Neither a missing disk
   nor conflicting lineage may be repaired by guessing from newest Config names.
5. Only that same real decided product may use old-or-new location recovery.
   Both/neither/wrong inode reject without adoption. Same-parent NOREPLACE and
   original ordered syncs preserve possible-effect InDoubt; no rollback or replan.
   CompletePending may finish the exact obligation, PreservePending may not move it.
6. Successful completion installs the exact effective-name successor; later
   Add/Exit/Backup and Current admission must preserve it through their existing
   real control source. No global freeze of those operations is a release shortcut.

The current code has no producer for steps 3-6. Therefore disk-name rename remains
unenabled. The immediately usable change is only stronger structural continuity
on the already existing preparation/recovery paths.

## Verification split

Three focused pure inventory tests cover shared B/A and C/B/A addresses, same
record under a different parent, and each of preparation/prefix/five-area
substitution. They do not manufacture a root, writer, successful rename or cold
release. Existing fresh ordinary/backfill/directory/FILE-rename tests stay selected
by their unchanged exact policy rules; default and lean product checks also remain.
No local build, test or selector execution is reported.

The first real disk-name test must add A rename then B rename to the actual
backfill fixture, cold reopen and an ordinary new BODY save after each completed
release. It must preserve original FILE/head/parent/tag/receipt identities and
C's actual readable BODY FD with upstream BODYs unavailable. Cuts before decision,
between B/C actions, after rename/sync and before completion must retain the exact
request and distinguish rejection from InDoubt. Target collisions, missing roots,
changed five-area inode and forged Config names must never become positive setup.
