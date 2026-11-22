# Build the single Linux executable

From an exact reviewed copy of the ZXS source tree on **Linux x86-64 GNU**:

```sh
python3 build_release.py /path/to/reviewed/rust-1.100.0/bin
```

The reference Rust compiler is 1.100.0, commit
`0ef1bc386059596dada8fb65b50425aab2ea637a`. Use a local trusted toolchain;
the build does not fetch it. Build as a regular user in a private worktree
under your exclusive control. Do not build on a deployed site or use a tree
concurrently modified by another process, even under the same UID. The
recipe refuses mounted/symlinked or untrusted output directories rather
than writing through them. The build host also needs Python ≥3.11, GNU GCC
with its static GNU C runtime, GNU ar/ld/readelf and the standard host tools.
The development reference used Debian GCC 14.2.0, binutils 2.44 and glibc 2.41.
Record the exact source revision/tree, the toolchain and C archive digests
printed by the build, and the resulting ELF digest. The printed tool and
archive facts are not an authentication of the toolchain. Compare digests only
with the same complete toolchain and source inputs. The recipe fixes the
target, Cargo lock/offline mode, archive epoch and build path remapping; the
C input versions alone are not a promise of equal package bytes.

The recipe builds only the `zxs` binary with local vendored code, including
SQLite and Argon2. It checks the ELF header and dynamic/program headers
without starting the program: x86-64 static PIE, no `PT_INTERP`,
`DT_NEEDED`, search/audit/filter dependencies or executable stack. It
writes one build output under this worktree, `target/standalone/zxs`, and
refuses to overwrite it. No Helper checkout, companion executable,
system service, installer, network fetch or archive refresh is needed.
Use a distinct clean worktree to reproduce the build separately. The fresh
host installation default is `/zxs/bin/zxs`, not the build output or working
directory. This recipe does not create `/zxs` or install the executable.

A digest and an ELF check are build evidence, not installation approval or
SMB/Web acceptance. Fresh site setup still uses independent administrator
approval, the exact fresh v18 shared-account SYSTEM and the original
protected deployment/site records. No v17 pool is upgraded. An optional
Helper must embed the complete unchanged bytes, not confer trust by hashing
an arbitrary installed executable. The primary runtime qualification is
separate on the exact accepted Linux target and newly prepared test pool;
this step never starts it or opens a deployed pool.

## Stage the browser resources separately

The browser crate is independent of the native Cargo build. From this ZXS
directory in a private reviewed worktree, with the same local Rust 1.100.0
toolchain described above:

```sh
python3 build_browser.py /path/to/reviewed/rust-1.100.0/bin \
  --wasm-std /path/to/rust-std-1.100.0-wasm32-unknown-unknown.tar.xz \
  --wasm-bindgen /path/to/wasm-bindgen-0.2.129-x86_64-unknown-linux-musl.tar.gz \
  --crates /path/to/local/crates-io-archives
```

Prepare the inputs before building:

- [Rust wasm32 standard library, 1.100.0](https://static.rust-lang.org/dist/2026-11-12/rust-std-1.100.0-wasm32-unknown-unknown.tar.xz)
  (SHA-256 `19b7954d650fc0d22767eceb76b8e03d07ce0d6a9f4234f33c32a17ee8d6d7b9`).
- [wasm-bindgen CLI, 0.2.129 Linux x86-64 musl](https://github.com/wasm-bindgen/wasm-bindgen/releases/download/0.2.129/wasm-bindgen-0.2.129-x86_64-unknown-linux-musl.tar.gz)
  (SHA-256 `82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e`).
- The official crates.io `<name>-<version>.crate` archives named in
  [`browser/Cargo.lock`](browser/Cargo.lock), in one local directory. The recipe
  checks every selected archive against its lock checksum and preserves its
  license files when staging it for Cargo.

The recipe checks the reviewed host toolchain's version, the archive pins
and the extracted CLI hash. Cargo builds a hashed private copy of the
manifest, lock and browser source, with a private target sysroot and Cargo
home. Local build scripts are not inputs. The build is locked and offline;
it does not modify the trusted toolchain. It writes `target/browser-assets/`
without replacement: `boot.js`, `folder.css`, `zxs_browser.js` and
`zxs_browser_bg.wasm`. Build-only `provenance.json` records the source and lock
hashes, tool inputs, and byte count and SHA-256 of each asset. Keep the reviewed
source revision and upstream license material with the build evidence.
Reproduce in a clean private worktree using the same complete inputs.

This command builds only the browser component. The native recipe above is
unchanged. When embedding is enabled, match the receipt to the reviewed build
inputs, check the four asset hashes and embed only those four files in the ELF.
