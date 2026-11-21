# Private fresh-only ZXS v18 issuer

This is paired development source, not a published release, migration, or
runtime acceptance result. The account commands are disabled by default.
When explicitly built with `private-fresh-zxs-v18`, the candidate uses only
the exact v18 SQLite schema (SHA-256
`8379dfa3b66040540995ebf863d3f12c7fddbb57ce05e98961011dc7429dfc4b`).
It never opens a v17 pool as this format. There is no account adoption or
automatic upgrade.

The intended later acceptance fixture is a **disposable Linux environment**:
already mounted and properly protected original SYSTEM (S), independent
recycle (R) and ordinary (F) roots; the explicit non-root storage UID/GID; an
empty protected `/var/lib/zxs/rust-smb`; and independently approved bytes of
the static ZXS ELF already at the explicitly chosen protected canonical
`.../zxs` file. The live Helper and its exact
offline libgcrypt/libgpg-error inputs also need independent expected hashes.
There is no installed Helper profile, configuration, service, or signing
sidecar as authority. These offline libraries are not ZXS runtime
dependencies. This source does not install an ELF, mount or format devices,
manage services, change policy, or embed a placeholder asset. The independent [optional image
extractor](embedded-image.md) requires a separately approved exact asset
and does not itself issue a site or an account.

1. `install-pool new-zxs-v18` creates only the pool: explicit original INIT
   actor, storage UID/GID, non-secret operation and approval IDs, actual
   S/R/F handoff. The separately lowered installer returns the committed
   pool/INIT/request SHA-256 only after complete original-marker readback.
   This is a separate storage action, not account setup.
2. `setup-pair issue-fresh-zxs-v18` takes that exact existing INIT evidence,
   explicit SYSTEM, original first LAN/share plan, separate required
   protected-site/operation IDs, the actual absolute `--zxs-image` path
   with fixed `zxs` leaf, and independent ELF/provider hashes. That path
   and digest are kept only in the original protected ZXS site. It
   accepts **one** canonical `--login`, shared by Web and SMB; no alternate
   username or password option. One preview binds the proposed account,
   exact public successor and original plan. Only one setup approval is
   taken at the actual foreground terminal. Afterwards, enter and confirm
   the **same** hidden password once, not one per protocol.

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

Isolated unit fixtures exercise exact public vectors, synthetic SQLite
transactions, closed low-peer framing and disposable non-secret PTYs. They
do not prove actual protected media, live callbacks, low writer capabilities,
cold startup, independently unchanged installer removal or real signed SMB
and Web logins. Those require later, agreed paired acceptance. No site or
credential operation is performed by the fixtures.
