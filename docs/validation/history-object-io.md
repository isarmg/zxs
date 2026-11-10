# Existing immutable history-object barriers

Normal `workspace` CI invokes `.github/scripts/check-history-object-io.py`. It builds the engine libtest with Cargo JSON output, selects its exact artifact (no stale binary glob), then calls `test/engine/unit/history_package/immutable_io/run.sh`. The shell preloads only each selected test child; no production process or global environment is changed.

The Rust test first installs an empty `state-page` on three configured members using the real immutable writer, then writes exact target paths and enables an observation marker. The C interposer observes libc `fsync` on those exact regular files. A successful target file barrier arms only that member's exact parent-directory barrier; unrelated kind/shard directory syncs are not counted as this object's durability. The selected single-thread test performs no unrelated object stores in this armed interval.

Seven children cover success, and one-shot file/directory EIO on each of the three members. Logs distinguish attempted, successful and injected-failure calls. Exact log assertions prove the interposer was actually reached, retain the first error's EIO source, and require full barriers both when retrying the same plan and when preparing again. The successful child also stores an undeduplicated two-reference control to establish the difference from the unique plan.

This checks the **existing-target** branch: one successful unique store on three members must emit three successful file barriers and three successful directory barriers. It is not a count of all process syscalls, raw rustix calls, initial directory creation, or physical power-loss behavior. No callback count substitutes for these observed libc calls. Fresh-object atomic creation retains the separate platform `atomic-io.sh` suite.

Run after building via the Python CI entry point, or pass an already-built engine libtest binary to `test/engine/unit/history_package/immutable_io/run.sh`. Missing observation, unexpected events, real I/O failure or unpreserved error sources fail the test. Merely compiling these files is not an executed durability result.

After every assertion the Rust child writes its case-specific completion marker. The shell requires its exact stage/member identity in the fresh case directory; libtest exit 0 with zero matching tests cannot pass. The CI command-suite tests execute zero-test and silent-success fake binaries and require the specific missing-completion diagnostic after proving the child ran.
