# Unified membership successor admission

Status: implementation WIP, not compiled or exercised locally. Remote targeted
validation is required. This does not claim backup, Exit/readmission, or complete
SYSTEM-loss recovery after membership activation.

## Authority

New unified Add intents use explicit `unified-genesis-v3` or
`unified-successor-v3` origins. Their reachable, hash-bound origin evidence contains
the original configuration, selector, and exact bootstrap Candidate/publication.
Existing bootstrap records and receipts are never rewritten. No missing evidence
is defaulted, and old v1/v2 roots are not migrated.

The portable verifier checks the original selector's full configuration commitment,
bootstrap publication, membership, operation and root-registration commitment before
accepting a retained unified seal. Physical original member identities are checked
against that selector, not inferred from a filename or a decoded current Config.
The independently verified frozen generation history must contain the exact initial
Candidate and retain its original operation result and principal.

A runtime-only `UnifiedSuccessorAdmission` binds that origin to the verified
Generation and the currently admitted SYSTEM carrier completion. A current disk
seal is `unified-storage-membership-successor-v3`, carrying both the origin and the
current generation material commitments. Legacy discovery/import paths do not
reinterpret this seal as old managed storage.

## Cold order

Cold open first retains the SYSTEM selection and validates SYSTEM-only carrier
completion, including writer and locator identities. It then holds the original
locator writer and uses only that locator's allowed probes to read the EXISTING
DISK `GENESIS_CONFIG` anchor. The original selector authenticates all configuration
bytes. This adds no independent configuration log or SYSTEM Genesis file.

Restricted discovery validates the membership certificate, archive, original
bootstrap evidence and physical members before live resource opening. The
resulting generation proof, not the mutable SYSTEM Config projection, chooses the
new member authority. Original RootRecord installation checks continue to cover
exactly their original roots; new roots are owned by the authenticated Add.

## Validation and remaining scope

`unified_membership_ancestor_tests.rs` contains four tests: retained bootstrap
operation/principal, SYSTEM-only preflight without storage probes, forged/missing
origin evidence, and actual FILE publication followed by Add activation and cold
reopen. The last test retains original proof bytes and tests missing-marker refusal
without filesystem changes. All eight membership reservation regressions remain
selected separately and keep their original positive assertions.

No local compilation, tests, formatter or selector was run. The current read of
Genesis material requires an eligible original root; absence is an error, never a
claim of nonexistence. Replica-degraded and interrupted Add paths, repeated Add,
recovery-carrier-plus-Add combinations, and SYSTEM loss after an Add require remote
behavioral evidence and further protocol work. SQLite/WAL I/O is not asserted to be
fully descriptor-relative by this change.
