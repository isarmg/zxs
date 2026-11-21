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
writes one executable under this worktree, `target/standalone/zxs`, and
refuses to overwrite it. No Helper checkout, companion executable,
system service, installer, network fetch or archive refresh is needed.
Use a distinct clean worktree to reproduce the build separately.

A digest and an ELF check are build evidence, not installation approval or
SMB/Web acceptance. Fresh site setup still uses independent administrator
approval and the original protected v17 deployment record. An optional
Helper must embed the complete unchanged bytes, not confer trust by hashing
an arbitrary installed executable. The primary runtime qualification is
separate on the exact accepted Linux target and newly prepared test pool;
this step never starts it or opens a deployed pool.
