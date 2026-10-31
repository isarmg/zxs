#!/usr/bin/env bash
# Compiler diagnostics, real target SDK FFI tests, and the required unwind policy.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${ZXS_SMB_SDK_SOURCE:?}"
: "${ZXS_SMB_SDK_BUILD:?}"
cache="${ZXS_SMB_SDK_CACHE:-$PWD/target/smb-debian13}"
mkdir -p "$cache"
python3 .github/scripts/check-rust-diagnostics.py -- \
  cargo clippy --locked --offline --message-format=json \
  -p smb-vfs --all-targets -- -D warnings
# Test harness output is not Cargo JSON. Gate compilation first, then run the
# already compiled tests without filtering their normal stdout/stderr.
python3 .github/scripts/check-rust-diagnostics.py -- \
  cargo test --locked --offline -p smb-vfs --no-run --message-format=json
cargo test --locked --offline -p smb-vfs
# Exercise the actual release crate with the configured SDK, not the test
# harness (which selects unwind independently). Reject only our expected gate.
abort_log="$cache/vfs-panic-abort.log"
if cargo rustc --locked --offline -p smb-vfs --release --lib -- -C panic=abort >"$abort_log" 2>&1; then
  cat "$abort_log"
  printf 'SMB VFS unexpectedly accepted panic=abort\n' >&2
  exit 1
fi
cat "$abort_log"
grep -Fq 'ZXS SMB VFS requires panic=unwind to isolate panics at the host boundary' "$abort_log"
# Finish with the supported release build; an unrelated negative build failure
# cannot pass the expected diagnostic gate or replace this positive check.
python3 .github/scripts/check-rust-diagnostics.py -- \
  cargo build --locked --offline -p smb-vfs --release --message-format=json
