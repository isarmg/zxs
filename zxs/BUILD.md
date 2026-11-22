# Build the single Linux executable

From an exact reviewed copy of the ZXS source tree on **Linux x86-64 GNU**,
first [stage the browser resources](#stage-the-browser-resources-separately)
and verify the single stage pin independently. Then build the release:

```sh
python3 build_release.py /path/to/reviewed/rust-1.100.0/bin \
  --browser-assets "$(pwd -P)/target/browser-assets" \
  --browser-asset-pin "$REVIEWED_STAGE_SHA256"
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
Record the exact source revision/tree, the browser stage pin, the toolchain
and C archive digests, and the resulting ELF digest printed by the build. The printed tool and
archive facts are not an authentication of the toolchain. Compare digests only
with the same complete toolchain and source inputs. The recipe fixes the
target, Cargo lock/offline mode, archive epoch and build path remapping; the
C input versions alone are not a promise of equal package bytes.

The release always includes the four browser resources in the same `zxs`
binary. The build checks that the receipt matches this browser source and
these two build scripts, then verifies the staged bytes against the separately
supplied pin. After either source or recipe changes, restage and review the new
pin; the release has no asset-free fallback. The [SYSTEM engine](src/system/store/ORIGIN.md)
and [password verifier](src/password/verification/ORIGIN.md) build from internal C sources.
The recipe checks the ELF header and dynamic/program headers
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
approval, the exact fresh schema-19 shared-account SYSTEM and the original
protected deployment/site records. This build does not upgrade earlier pools. An optional
Helper must embed the complete unchanged bytes, not confer trust by hashing
an arbitrary installed executable. The primary runtime qualification is
separate on the exact accepted Linux target and newly prepared test pool;
this step never starts it or opens a deployed pool.

## Stage the browser resources separately

The same `storage-server` Cargo package builds the WebAssembly module from
`src/browser/`. In a private ZXS worktree with the same reviewed local Rust
1.100.0 toolchain as above:

```sh
python3 build_browser.py /path/to/reviewed/rust-1.100.0/bin \
  --wasm-std /path/to/rust-std-1.100.0-wasm32-unknown-unknown.tar.xz
```

Use the official [Rust wasm32 standard library, 1.100.0](https://static.rust-lang.org/dist/2026-11-12/rust-std-1.100.0-wasm32-unknown-unknown.tar.xz)
(SHA-256 `19b7954d650fc0d22767eceb76b8e03d07ce0d6a9f4234f33c32a17ee8d6d7b9`).
The browser target has no third-party dependencies or generated bindings.

The recipe checks the host toolchain and target archive. It uses the root
manifest and lock in the original source worktree, with a private target
sysroot, Cargo home and output, and rechecks the browser source and entry
inputs. The build script skips native C and embedding on wasm32. This
build never fetches dependencies or modifies the toolchain. It writes
`target/browser-assets/` without replacement: `boot.js`, `folder.css`,
`zxs_browser.js` (the checked-in browser host code), and
`zxs_browser_bg.wasm` (the Rust console). Build-only `provenance.json`
records source, recipe, tool and output digests. Reproduce in a clean
private worktree with the same inputs.

This command builds only the browser component. To check the compiled module
with the synthetic browser host (no server), run
`node tests/browser/console-host.mjs target/browser-assets`.
Match the receipt to the reviewed build inputs and verify the four asset hashes before approving a
stage pin. Keep the stage and source worktree under your exclusive control.
Keep the reviewed pin separate from the stage and supply it explicitly to the
release command; a digest calculated from an unreviewed directory is not
approval.

The binary stage pin covers `boot.js`, `folder.css`, `zxs_browser.js`,
`zxs_browser_bg.wasm` and `provenance.json`, in that order. Hash the ASCII
domain `ZXS-BROWSER-ASSET-STAGE-1` plus one NUL, then each file's one-byte
name length, name bytes, eight-byte big-endian length and 32 raw SHA-256
bytes. Compare the stage pin against independently reviewed inputs, not a
value supplied by the stage itself. Only the four fixed resources go into the
ELF; `provenance.json` is not served.

### Direct native Cargo source checks

A native ZXS build or source check needs the same reviewed browser stage
and pin. For a source-only check:

```sh
ZXS_BROWSER_ASSETS=/absolute/path/to/target/browser-assets \
ZXS_BROWSER_ASSET_PIN="$REVIEWED_STAGE_SHA256" \
cargo check --lib --locked --offline
```

This check does not produce the supported release artifact.
