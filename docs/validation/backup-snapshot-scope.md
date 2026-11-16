# Whole Backup and immutable directory snapshots

Whole Backup currently transfers FILE histories and their original BODYs. It
does not transfer independently registered `.zxs` immutable snapshot nodes to
new target occurrences. Copying the snapshot's SYSTEM receipts alone cannot
make that incomplete transfer a complete backup.

The production boundary uses the complete existing `ReceiptIndex` over the
admitted State, including exact operations, principals, events, reserve and
completion relationships. It does not inspect a `.zxs` directory name, infer a
snapshot from directory Git configuration, or acquire a source from a local
manifest. The index preallocates completion IDs at reserve time, so membership
of its completion map is deliberately **not** the publication test.

* A validated completed initial or committed immutable node returns
  `UnsupportedOperation`, identifying its permanent publication.
* An actual node reserve without completion returns `RecoveryRequired` with
  the existing typed registration cause, not a fictitious published snapshot.
* Invalid registration preserves its original typed cause and error code.
* A State with no registered snapshot remains eligible for the pre-existing
  whole-Backup source and material checks. No permission is issued by this gate.

The common source admission is before single-task, batch-task and user
transaction intent/target writes. Both copy resumes and native preparation use
that same source admission. Frozen snapshot creation and target-local snapshot
read apply the same boundary **after** their existing full publication/object
authentication. Thus a previously retained target cannot pass an offline whole
completion report or later native stage merely by carrying receipts for omitted
immutable snapshot payloads. Raw local inspection is not a runtime authority.

## Actual selected regression

`backup_snapshot_scope_tests` creates a registered three-tier pool and a real
ordinary FILE, initializes a nonempty immutable snapshot through the public
Engine API, and reopens its exact registered node/BODY. Single, batch and user
transaction starts must all return the typed rejection before any SYSTEM or
target write. Full file bytes, modes and device/inode identities are compared,
along with the same visible State allocation. The entire rejection is repeated
after actual cold reopen and exact snapshot request replay.

The second case stops the actual namespace-host and node-reserve publisher
before physical node preparation or completion. It proves the allocated
completion key already exists while the result is specifically pending, and
that Backup performs no writes. Neither test fabricates a successful receipt.

The exact two tests are selected in default and fault-injection configurations
with a 300-second bound. The original independent terminal serving/successor
test and its 900-second default/fault-injection bound are unchanged. These are
source-level expectations until their exact hosted run executes.

## Positive transfer remains open

This rejection is a safety boundary, not completed directory backup support.
Future support must enumerate actual immutable snapshot files/SMB names from
the independently registered node and copy them through a held source to each
target's own source-bound directory occurrence. Target entity creation,
durable feedback, Candidate/decision binding, publication, restart and exact
readback must close together. Original source inodes cannot be reused as target
identities; FILE databases, labels and unrelated history are not substitutes
for the immutable snapshot's permitted file material. No new wire format,
compatibility branch or speculative target writer is introduced here.
