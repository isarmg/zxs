# Guarded disk read lifetimes (experimental)

This batch is not a completed disk offline, downline, deregistration or restore
operation. It introduces process-local ownership for selected managed disk reads
and tests cancellation through a fault-injection-only entry point. There is no
production administrative endpoint that marks a disk disabled.

The managed object is a registered ZXS storage directory, not the operating
system's block device. A metadata member is a role of that storage directory.
Directories on the same filesystem are supported; distinct device numbers are
not required. This work does not format, mount, unmount or disable OS devices.

## Implemented boundary

- Verified physical disk bindings share an admission gate across Engine instances.
  A new handle cannot reset an existing gate. Certified membership activation
  validates every affected gate before advancing any of their control contexts.
- Managed file descriptors carry ownership until their final real descriptor is
  closed. Duplicates are registered too. Revocation closes idle descriptors without
  waiting for the HTTP body or SMB reader to poll or be dropped. An in-flight
  syscall retains ownership until its actual return and cleanup.
- Managed engine reads, HTTP streaming and SMB streaming/cache access preserve
  that ownership. Each fallback source has independent admission. Cancellation is
  terminal for a revoked source and cannot become an endlessly retried interrupted
  read. Local workspace/package files remain explicitly local.
- Genesis control-context equality includes the complete persisted configuration,
  excluding relocatable root paths and array order. This is an in-memory equality
  check, not authentication or a durable membership decision. Successor control
  authority is tied to the admitted generation.

## Required validation

The experimental workflow builds each engine/server unit-test binary once and
uses explicit nonempty groups. It retains exact-commit and binary-hash evidence,
failed groups, production diagnostics, strict Clippy and formatting results. The
engine groups include membership activation, recovery, proofs, history, scrub,
disk admission, authority equality, reads and verified payloads. Server groups
cover HTTP transfer plus SMB stream reads, bounded lifecycle and close handling.
Production server builds are checked both with and without its core feature.

New cases cover idle/backpressured consumers, actual descriptor closure,
in-flight ownership, duplicate descriptors, shared gates across Engine reopen,
stale control contexts, configuration differences, all-or-nothing authority
advancement, healthy fallback and repeated cancellation. Temporary files are
synthetic fixtures, not physical-device or real-filesystem deployment tests.

At preparation time these changes have only been reviewed and formatted. No local
build or test has run. A GitHub run must pass for the exact final commit before
any validated result is claimed; this document is not itself a pass report.

## Remaining implementation

The next [bound-root synchronous operation slice](guarded-root-operations.md)
accounts synchronous root calls, but raw returned resource lifetimes, journals,
writer locks, uploads and cleanup paths are not yet comprehensively governed
by this read gate. Writer locks must remain held
through fencing, not be revoked as ordinary readers. A production coordinator
still needs controlled transition IO, durable prohibit-IO state loaded before
startup probes, certified old/new voting changes, and verified recovery.

Offline/downline must retain registration and data while prohibiting disk IO.
Deregistration must instead remove current registration and affected current
visibility, retain the disk's files, and durably place a verified restoration
record at that disk's root first. Shared content and historical proofs must remain
intact. Those operations and their API/Web integration are not implemented by
this batch. No data deletion or physical disk operation is authorized by the
test hook.

## Prepared payload proof ownership (source pending verification)

Retained payload proofs now keep their body, version/checksum/payload metadata,
and disk-seal files in the same revocable managed-file owner used by reads.
Metadata stamps are taken from metadata acquired under that owner; they never
require exposing the inner File or descriptor. Both cached proofs and the
uncached full-revalidation path reject a quiesced resource. An idle proof can
remain allocated without retaining an open target descriptor. Its optional
process-wide descriptor-budget reservation remains allocated until the proof is
dropped; that cache budget is not a claim of outstanding disk I/O.

Four new test sources cover idle cached proof closure, uncached retained seals,
fresh preparation after quiesce, and cancellation after the final replica in
both modes. The existing mode-change regression still changes the exact fixture
inode through its test path rather than making the read wrapper writable. The
metadata-stamp field/equality test also checks the guarded-metadata constructor.
No local compilation or test execution was performed; these require the next
GitHub validation batch. This does not migrate raw copy sources, journals,
private-directory publishers, writer locks or root descriptors, and does not
complete production offline or deregistration.

The generic internal `ReadSource::open` now returns the same owned `ReadFile`
as explicit leased reads. Managed workspace sealing, staged-range copy inputs,
recovery sources and the first publication copy can no longer select a raw-file
alternative. Publication's subsequent replica-copy source is also opened through
the producing root's gate. External package/system inputs remain explicitly
local. A new source/clone test checks cancellation and failed reopen. Destination
writers and legacy raw repair probes remain separate, unfinished coverage; this
change does not claim a complete write-side drain. Source inspection and direct
formatting only, pending GitHub execution with the other affected groups.

## Scrub and existing-payload read ownership (source pending verification)

The payload-opening helpers used by degraded recovery, repair verification and
scrub now retain `ReadFile` ownership rather than returning a raw `File`.
`DiskRoot::open_read_regular` preserves the root's already established provenance:
a bound managed root must obtain its gate; an explicitly unbound bootstrap or
offline root remains local. Admission failure cannot select the unbound branch.
Checksum reads and retained `VerifiedExisting` body proofs use the same owner.
Their identity comparison uses guarded device/inode metadata without exposing a
descriptor. Disk-local repair source matching is also leased.

Four new test sources cover all three payload-opening consumers, a retained
existing-body proof, cancellation during verification, and an idle scrub batch
whose revoked source closes without cancelling the other two copies. The file
consumer tests check real `/proc/self/fd` closure and unchanged fixture bytes.
Root admission tests also cover rejected managed opens and explicit unbound
fixture reads. Existing copy, repair and scrub regressions remain required.
No local build or test was run. Destination writers, journal/maintenance handles,
root descriptors and writer locks remain separate implementation work, so these
changes still do not complete an administrative offline or deregistration.

## Read-only journal consumers (source pending verification)

Scrub metadata scanning and control-archive journal inspection now open their
source journals through the root's owned read path. Control-archive journal copy
inputs retain the same provenance. These changes do not revoke or release active
journal writers, writer locks, destination archive publishers or root handles.
Two new test sources compare actual read-descriptor closure with the still-held
writer, require cancellation failure, and preserve the original journal bytes.
All execution remains pending the next GitHub validation batch.
