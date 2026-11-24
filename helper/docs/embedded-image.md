# Optional raw Helper with the exact standalone ZXS image

The normal Helper has no placeholder payload. Build the optional separate
Helper only with the already reviewed and independently measured **complete**
standalone ZXS ELF from the independently selected current ZXS source. Do not use
an old-format or development stand-in. This action neither releases
nor starts either program.
The final runnable ZXS remains that one unchanged file; it never calls Helper.

In a trusted private Linux x86-64 GNU **build** worktree as an ordinary user,
use Python 3.11 or later, the approved local Rust 1.100.0 toolchain, standard
GNU compiler/linker tools, and a previously verified offline vendor directory
for this exact Helper Cargo.lock:

```sh
python3 build_embedded.py \
  --rust-bin /approved/toolchain/bin \
  --offline-registry /approved/locked-helper-vendor \
  --zxs-elf /approved/standalone/zxs \
  --zxs-sha256 INDEPENDENT_WHOLE_ZXS_SHA256 \
  --zxs-source-tree EXACT_REVIEWED_ZXS_TREE_SHA1
```

Inputs are named canonical local paths, not auto-fetched dependencies. The
script makes a private exact-byte copy, independently checks the approved
input hash and static PIE shape, builds locked/offline with a fresh private Cargo
home and only its generated offline registry configuration (no source or
ancestor Cargo config), and verifies that
the actual Helper ELF contains **one whole unchanged payload** in one read-only
loadable region, with no other `PT_LOAD` mapping over its Linux x86-64 virtual
pages, including a load's zero-filled memory range. See the
[Linux ELF loader](https://github.com/torvalds/linux/blob/v6.12/fs/binfmt_elf.c#L329-L413)
and [x86 page size](https://github.com/torvalds/linux/blob/v6.12/arch/x86/include/asm/elf.h#L210-L214).
It does not execute either image. Only the raw
`target/standalone/helper` artifact is exported, without overwriting. Its
receipt reports both full hashes and records, but does not authenticate, the
supplied ZXS source-tree identifier. The optional Helper has its normal Linux
host tool requirements; the embedded ZXS product is static independently.

## Explicit extraction, not a site or service install

Only the optional embedded build can offer:

```sh
./helper install-zxs-image --zxs-sha256 INDEPENDENT_WHOLE_ZXS_SHA256
```

A supported Linux host, real root foreground terminal, and the already
prepared protected `/zxs` and empty ZXS-owned `/zxs/state` (both root:root
0755 with no access/default POSIX ACL) are required. Run extraction **before** creating `/zxs/state/headless` for first setup: the protected state parent must still be empty. This action does not create that parent or take over an old site.
The default writes `/zxs/bin/zxs`; `--destination-dir DIR` may select another
already existing, eligible protected local directory distinct from the ZXS
state, log and media trees. Only the fixed leaf `zxs` is allowed. It requires an independent expected digest, exact payload
verification and a fresh foreground `INSTALL-ZXS-IMAGE` approval showing
the actual destination and digest. No unattended entry or fallback.

Before publication only one unnamed original temp file is written on that
same mount. The actual filesystem and root capability must support unnamed
temporary files and `linkat(AT_EMPTY_PATH)`. The authorizer rejects readonly
or noexec mounts and unexpected ownership, aliases, or ACLs. Full bytes and
EOF are verified before publication; it is no-replace, and the exact new
published inode is independently reopened, hashed and synchronized with its
original parent. If publication is uncertain, keep the old and new evidence
as found and do not blindly retry or delete. The command never starts ZXS,
writes site/account state, resets a credential, migrates a pool, or
overwrites an existing file.

The separate [current first setup](headless-offline.md) uses the dedicated
non-root service account, not this root publisher. Its default installed
program location, also used by offline name lookup, DATA initialization and
imports, is `/zxs/bin/zxs`. Extracting to a different protected directory
does **not** reconfigure those commands. Extraction grants no account and
creates no role markers or `server.conf`. The `embedded-zxs` build feature only
adds the explicitly selected payload and its independent extraction entry.
