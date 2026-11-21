# Fresh standalone Rust SMB site boundary (private source)

This is an internal phase of the same foreground ZXS program. The normal ZXS
start selects its original deployment; there is no separate SMB binary, public
SMB command, general server API, or independently configured SMB service.
This boundary never scans host disks, adopts an existing Native site, uses
Helper discoveries as authorization, or reads any /etc/local-ops-helper file.
The paired fresh issuer must create the protected ZXS-owned deployment record
once after genuine shared activation. This runtime does not provision or
upgrade storage, including any v17 split-account pool.

## Existing protected originals

- The normal no-argument entry selects only
  /var/lib/zxs/rust-smb/deployment.v1, root:root 0600 with one link.
  It is exactly 64 bytes: ZXS-DEPLOYED-V1 plus NUL (16 bytes), the instance
  ID (16 bytes) and SHA-256 of the exact site.v1 (32 bytes). Keep the
  original deployment-directory shared lock, then SYSTEM shared and the
  exact site exclusive; the fresh issuer owns the matching order. No
  scan, default site, manual SMB command, or legacy environment fallback.
- /var/lib/zxs/rust-smb/<instance-hex>/ is an actual directory for the exact
  32-lowercase-hex InstanceId: root:<storage GID> 0750, exactly rust.install,
  site.v1 and vault. Trusted root-owned ancestors deny group/other writes and
  path substitution. Newly planned layout and legacy Helper-only
  /var/lib/local-ops-helper/... are different cases; there is no copying,
  implicit selection, or fallback.
- rust.install is the original ZXS-RUST-SMB-I1 public binary descriptor,
  root:root 0444 and one link. It already defines host/one SMB share, exact
  LAN endpoints and prefixes, earliest folder names/modes and recycle label.
  A separate lower-privilege reader independently qualifies its entire shape
  and original v18 shared-account receipts; this root selector recognizes only the matching
  five IDs and the selected whole SHA-256.
- site.v1 is exactly one root:root 0600 ordinary, one-link file on that site.
  Binary bytes in order: ZXS-RUST-SITE-1 plus NUL (16 bytes); Pool, Init,
  Instance, Bind, Setup (five 16-byte IDs in descriptor order); actual
  historically approved INIT actor (16 bytes); two u32 big-endian nonroot
  UID/GID; SHA-256 of exact rust.install; independently approved SHA-256 of
  the entire same ZXS ELF; twice a u16 big-endian counted length followed by
  its UTF-8 bytes: the exact absolute SYSTEM directory and exact installed
  ELF path, respectively. Each path is 1..4095 bytes, canonical, bounded
  by component/depth, and must not overlap the site or the other path.
  No terminator, optional field, data mount number, login, password, policy
  cache, or path fallback. Parsing alone is no approval.
- vault is original same-mount root:root 0700. Root origin selection never
  reads its credential body. Once the original low socket/pidfd worker
  reports exact ACTIVE (not merely a tuple), the typed launch opens the
  original protected one-link 240-byte credential.v2 RO for the proven
  lower-privilege SMB role. No vault FD goes to Web and no Web verifier to
  the parent or SMB. The sole active account epoch is in SYSTEM.

Root selects site.v1, acquires the lifetime original SYSTEM SH before the site
EX, rechecks original name/inode/mount and profile, obtains only public
configuration and never opens SQLite. The approved named executable must be a
plain root:root 0755 file with no capabilities/ACL, exact independent whole
hash, valid static ELF form, and the actual current executable inode.
PT_INTERP, DT_NEEDED, loader-path and audit dependencies are not accepted.
No dynamic fallback or release package is produced. File structure/digest
alone cannot demonstrate independence from late host calls; the single-ELF
build and disposable, no-Helper cold launch must establish that separately.

The same foreground terminal renders only the authenticated low ACTIVE facts
and the actual retained media selection, then issues one typed confirmation
consumed by the same ZXS instance, reply and selected view before listeners or
the serving handoff. Web always uses the original HTTP listener on
0.0.0.0:8443. The real foreground consent selects direct cleartext or the
strict same-host authenticated proxy mode for this run only. The public
origin/digest do not alter the site record, and only the sealed Web START
carries that scope. Existing proxy environment flags are rejected; no
malformed or failed proxy mode falls back to direct. TLS is external.
The Web child
gets the same original SYSTEM and selected ordinary media read-only for the
manual version proof, with no recycle, vault, or SMB socket. Both lowered
births are the same installed ZXS ELF, with separate original listeners and
receipts. Each independently proves the same exact S, PLAN, role and public
shared account epoch, then sends one authenticated PREPARED and EOF. After
both original receipts and a last liveness/TERM vote, the parent takes one
common irreversible step: F_SEAL_SEAL on its retained writable original
gate. Both child loans are genuinely read-only after exec, bound to the same
inode, commitment and hard monotonic budget. No userspace accept, challenge,
HTTP handler or SMB cold write task before that role verifies release. A
TCP backlog can exist before release. This is not a positive acknowledgment
that both loops processed requests or completed old-operation recovery.

Never print credential bytes or original volume tokens. Site, SYSTEM and
approved ELF identity stay held until actual drain/reap. Start both normal
stops before any wait; when the second birth fails, begin stopping the
already prepared sibling before failed cleanup can wait. No automatic
restart or fallback. SYSTEM and site mode or identity changes are not
repaired at startup. Mount observations are host-local and contemporaneous,
not durable hardware claims or protection from a malicious host administrator.

Future optional Helper: one explicit fresh session, existing protected
offline EX authority with lower-privilege initial writer, exact embedded
standalone ZXS bytes and explicit new destination, no implicit overwrite or
reinitialization. No Helper state is read by this runtime contract. This
document enforces no migration, installs nothing, and is not release approval.
