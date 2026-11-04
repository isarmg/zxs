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
