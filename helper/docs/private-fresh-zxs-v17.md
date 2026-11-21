# Private fresh-only ZXS v17 issuer

This is an isolated source candidate, not a shipped Helper command, product
package, migration path, or runtime acceptance result. The default Cargo
features do not accept either command. Even when deliberately built with
`private-fresh-zxs-v17`, this code requires schema 17 and the exact paired
DDL SHA-256 `a5eb88872ac976929b6ca4e5621a74de979ac9040512d1d937b50452307f42d0`.
Do not use with a v16 installation, existing media, or an existing site.
This exact source route is separate from formal release or existing installations.

The intended later fixture has a **disposable Linux environment**, blank
explicitly selected protected SYSTEM/recycle/data directories (recycle with
independent backing), one explicitly approved nonroot storage UID/GID from
that environment, a protected **empty** `/var/lib/zxs/rust-smb`
(root:root 0755, no ACL/adoption), and the independently approved exact static
ZXS ELF already at `/usr/bin/zxs`. This candidate does not create or change
system parents or installation paths. It uses the pinned installed Helper and
its pinned offline libgcrypt and libgpg-error provider, not an external server, a
saved Helper profile, or a project signing sidecar. These providers are not
dependencies of the running ZXS ELF. Any FIPS/host/runtime policy refusal remains
a refusal. The future optional Helper with the complete embedded ZXS binary is
not part of this slice.

There are two explicit foreground approvals, each using the actual retained
terminal, plus the original separate credential confirmation and input:

1. The private `install-pool new-zxs-v17` variant takes the explicit
   original INIT actor, storage UID/GID, operation/approval, and the actual
   blank S/R/F paths. It uses the existing permanently lowered initial
   installer, not a new schema or recovery mode. It returns the committed
   pool ID, INIT operation and original INIT request SHA-256 only after
   independent readback of the original whole marker group.
2. The private `setup-pair issue-fresh-zxs-v17` variant takes that actual
   INIT actor/UID/GID, SYSTEM, pool, INIT and request SHA-256, distinct
   instance/Bind/SETUP and approval IDs, separately approved exact
   ZXS/Helper/libgcrypt/libgpg-error SHA-256 values, and original first
   plan/endpoints/share roles. Passwords never enter argv, the Root SQLite
   path, or the public deployment files. The existing independently lowered
   account writer stores the Web verifier; only the separately reaped lower
   credential worker receives the fresh vault writer and SMB password.
   No legacy environment or runtime role is an alternative.

Only markers may remain in the selected F/R when first issued. The protected original SYSTEM EX, original low-observed SQLite O_PATH inode
(non-readable and never used as a SQLite VFS FD), and verified recorded
mediums remain held during the issue. The dedicated product parent EX is taken before S EX (matching cold ZXS
lock order); the exact new instance EX then spans host publication. Only the original account-free
committed INIT and its exact historical request may start a writer. Original
pending/account/site records are never adopted. Every attempt is one-shot.

No ordinary ZXS process is launched here. The instance's `rust.install`
(root:root 0444), `site.v1` (root:root 0600), and protected empty vault are
staged and published without replacement. After the original vault has been
placed and read back, the actual low SQL worker confirms ACTIVE generation 1,
syncs the Unix SQLite and original parent, independently rereads, and exits
cleanly with EOF from its authenticated channel. Only then is a fixed 64-byte
`deployment.v1` selector staged, synced, published without replacement,
and independently checked against those same actual originals. Without that
selector a stray site is not a runnable deployment. The live consumer's
independent original SQL/vault/image/runtime checks still apply. Any error
after the issue confirmation means **outcome not confirmed**; preserve the
original identities and any stage or final names. Do not retry, remove,
auto-repair or interpret the presence of a filename as success.

Pure source fixtures named `fresh_v17_` use only the known wire vector,
synthetic in-memory ledger, disposable temporary directories and static ELF
header bytes. They do not call either CLI operation or operate on real
mounts, accounts, packages or listeners. A separate exact paired clean-VM
acceptance remains required: genuine originally protected fresh S/R/F,
issue once, then remove the standalone Helper and its offline providers,
cold-start the same verified ELF without a companion server, and exercise its built-in
Web and signed SMB login. This is not a release or an unattended boot flow.
