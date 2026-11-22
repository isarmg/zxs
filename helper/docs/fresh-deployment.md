# Private fresh deployment

A default Helper build rejects both commands below. For authorized fixture
validation, use the [independent
locked, offline Helper build](../RELEASE.md) with
`--features private-fresh-install`. The private route accepts only the exact
v19 SQLite schema (SHA-256
`5cf057fe1df72bd606ad47f11beec7747779a67206b781bab89d2aeeaf8c8495`), with no
migration, adoption, or automatic upgrade. Prior v18 catalogs are refused;
this Helper does not initialize or modify FILE labels.

The intended later acceptance fixture is a **disposable Linux environment**:
already mounted and properly protected original SYSTEM (S), independent
recycle (R) and ordinary (F) roots; the explicit non-root storage UID/GID;
an empty, pre-provisioned `/zxs/state`; and the independently approved standalone ZXS ELF at
`/zxs/bin/zxs` or an explicitly eligible protected canonical `.../zxs` file.
Both `/zxs` and `/zxs/state` are root:root 0755 with no access/default
POSIX ACL. `/zxs` may also be a separately mounted complete supported local
filesystem root (not a mounted subtree). No mount crossing is accepted
at `state` or below it. Sites belong to ZXS and are installed only
under `/zxs/state/<instance-hex>`; the sole selector is
`/zxs/state/deployment.v1`. The fixed parent/member checks require exact raw
names and a complete bounded listing (4,096 non-dot names and 256 KiB per
checked parent); they do not open unrelated entries. No prior deployment
path is probed or migrated.

Run neither command on production media. The fixture uses a real root
foreground controlling terminal in the disposable guest, with stdin, stdout
and stderr on that terminal, not redirected or piped. No ZXS service is
started by these steps. Linux must already provide the qualified mounts and
paths; the fixture does not partition, format, mount, install services, or
change host policy. The [optional image extractor](embedded-image.md) is a
separate approved action and does not issue a site or an account.

For account issuance, approve the whole-file SHA-256 of the actual Helper
(root:root, 0755, one link) and of the selected standalone ZXS ELF. The two
fixed offline credential providers must be protected root:root 0644 regular
files with one link: `/usr/lib/local-ops-helper/disk-tools-v1/lib/libgcrypt.so.20`
and `/usr/lib/local-ops-helper/disk-tools-v1/lib/libgpg-error.so.0`. Use their
independently approved file digests, not archive digests. The [Debian material
recipe](../packaging/debian13-amd64/README.md) stages these bytes; it neither
installs nor approves them. These libraries are not ZXS runtime dependencies.

Before account issuance, the inherited soft Linux `MEMLOCK` limit must
cover the 64 MiB password workspace plus the credential worker's other
locked memory. A limit of 64 MiB or less is rejected before writing any site
files. Helper does not raise the limit; the non-root worker still verifies
that it can lock and reserve the required memory before reading a password.

## 1. Create the fresh pool

The command forms below describe the required order, not copy-paste shell
commands. Brackets mark optional fields; repeat the indicated media options.

```text
helper install-pool new-deployment
  --init-actor INIT_ACTOR_ID --storage-uid UID --storage-gid GID
  --op INIT_OP_ID --approval INIT_APPROVAL_ID
  [--system S_DIR] [--recycle R_DIR] (--data F_DIR | --data-label DISK_LABEL) ...
```

Give 1–62 explicit ordinary roots and prepare S/R/F first. The defaults for S and R are `/zxs/disks/system` and
`/zxs/disks/recycle`. The protected `/zxs` layout is checked before and after
the original foreground INIT approval. After successful marker readback,
retain the `fresh-init=confirmed` receipt fields `pool`, `operation` and
`init_request_sha256`, along with the original subject, paths and approval
reference for step 2. If the output was lost, `install-pool receipt` with the
same original INIT inputs returns the same fields when confirmed. A pending
result is not a setup receipt. See the [original-operation
guidance](sqlite-first-install.md); do not start another pool to obtain a receipt.

## 2. Issue the shared account on that fresh pool

```text
helper setup-pair issue-fresh
  --init-actor INIT_ACTOR_ID --storage-uid UID --storage-gid GID [--system S_DIR]
  --pool POOL_ID --init INIT_OP_ID --instance INSTANCE_ID --bind BIND_ID --setup-op SETUP_OP_ID
  --init-sha256 INIT_REQUEST_SHA256 --approval SETUP_APPROVAL_ID [--zxs-image DIR/zxs]
  --zxs-sha256 ZXS_SHA256 --helper-sha256 HELPER_SHA256
  --gcrypt-sha256 GCRYPT_SHA256 --gpgerror-sha256 GPG_ERROR_SHA256
  --host HOST --login LOGIN --recycle-name RECYCLE_NAME
  (--listen IP/PREFIX | --share F_DIR NAME MODE | --share-label DISK_LABEL NAME MODE) ...
```

- Reuse the same INIT actor, storage UID/GID and exact SYSTEM path. Copy
  `pool`, `operation` and `init_request_sha256` to `--pool`, `--init` and
  `--init-sha256`. Choose new instance, bind and SETUP operation IDs. The
  five pool/INIT/instance/bind/SETUP IDs must be distinct. IDs are 32 lowercase
  hex digits and nonzero; SHA-256 values are 64 lowercase hex digits and
  nonzero. `--approval` refers to the separate setup approval.
- Use the already approved executable at `/zxs/bin/zxs`, or explicitly
  select an eligible protected `DIR/zxs` path. Always supply the independent
  image hash. The original protected site binds that path and hash.
- Plan 1–16 explicit SMB listen IP/prefix pairs (not wildcard or loopback)
  and 1–62 shares from ordinary roots in the original INIT. `DISK_LABEL`
  selects a directory under `/zxs/disks/data`; `NAME` is the share name and
  `RECYCLE_NAME` is the logical recycle name, not a path. `MODE` is `ro` or
  `rw`. No disk scanning and no listeners are started by this command.
- Use one canonical lowercase hostname and **one** non-guest login shared
  by Web and SMB. The original terminal gives one setup approval, then
  prompts for the same hidden password and its confirmation. There is no
  password argument or second username.

The root coordinator holds the original product parent, original S and then
instance leases. It grants a new write-only vault stage to the already
lowered, memory-protected password worker. One owned zeroizing input is
borrowed by both protocol derivations. Only the salted Web verifier takes a
single authenticated, sealed anonymous transfer directly to the independently
lowered original SQLite worker; permission to set an epoch never travels
with that payload. The SMB verifier is written only to the vault. The parent
handles public prefix and metadata proofs, never plaintext or a protocol key.

After the writer has exited cleanly, the original stage and original
directory are synced and independently rechecked. The SQLite worker verifies
the same original S and stage, then stores the Web row, public vault
coordinate and one pending shared setup. Both reader roles are closed while
pending. Only after placement and independent readback of the same actual
`credential.v2` inode can the second short transaction activate both roles
and the exact first shares. It independently reopens and rereads that shared
result after syncing. The parent verifies actual worker exit, EOF and
original named material. Only then does the no-replacement, synced
`deployment.v1` selector make the prepared site discoverable to ZXS.

Any error after the setup approval means the result may be unknown. Preserve
original IDs and staged/final names; don't restart, clear, retry with a new
ID, or infer success from a path. There is no cold recovery for abandoned
fresh account setup. The active reader must separately qualify the original
ledger, vault, static image and volume roots before serving.

Unit fixtures cover public vectors, synthetic SQLite transactions,
low-peer framing and disposable non-secret PTYs. Paired acceptance remains
pending for protected media, live callbacks, low writer permissions, cold
startup (including after Helper removal), and real signed SMB and Web logins.
