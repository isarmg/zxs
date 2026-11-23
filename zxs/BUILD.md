# Build the single Linux executable

From a reviewed ZXS source tree on Linux x86-64 GNU:

```sh
python3 build_release.py /path/to/reviewed/rust-1.100.0/bin
```

Build as a regular user in a private worktree with Python 3.11 or newer,
GNU GCC with its static C runtime, GNU ar, ld and readelf. The recipe uses
the supplied Rust 1.100.0 toolchain, commit
`0ef1bc386059596dada8fb65b50425aab2ea637a`, and the locked offline Cargo
inputs. There are no external Rust crates. The Linux ABI shims use the
target toolchain’s C and Linux headers; the
[password verifier](src/password/verification/ORIGIN.md) builds from source
in this project. No browser resources or installed SQLite library are needed.

The recipe writes one file, `target/standalone/zxs`, without replacing
an existing output. It inspects the x86-64 static PIE for a valid
non-executable stack and absence of an ELF interpreter or dynamic library
dependencies. Record the reported output SHA-256, source revision and
complete toolchain inputs when comparing builds.

The build does not start the server or configure a deployment. The
installed binary is selected separately and requires the protected
gateway and storage handoff. The release does not open or migrate a
legacy pool.
