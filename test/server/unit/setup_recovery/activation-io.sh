#!/usr/bin/env bash
# Isolated temporary-directory control durability checks. No real disks or credentials.
set -euo pipefail
binary="$(realpath "${1:?pass the storage-server libtest binary}")"
test_name=setup_recovery::activation::tests::published_control_requires_durable_retry_before_account_or_storage_admission
listed="$("$binary" --list --ignored --exact "$test_name")"
[[ "$(grep -Fxc "$test_name: test" <<< "$listed")" == 1 ]] || { echo "Missing exact ignored activation I/O test" >&2; exit 1; }
fixture="$(cd "$(dirname "$0")/../../../platform/unit/fs_safety" && pwd)"
# Discovery rejects tmpfs; keep only these isolated data directories on the workspace filesystem.
temporary="$(mktemp -d "$PWD/.activation-io.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/atomic_io.c" -ldl -o "$temporary/atomic-io.so"
for mode in management activation; do
  for stage in directory-sync published-file-sync; do
    base="$temporary/$mode-$stage"
    mkdir -m 700 "$base"
    destination=storage-activation-complete.json
    [[ "$mode" != management ]] || destination=management-initialization.json
    env LD_PRELOAD="$temporary/atomic-io.so" ZXS_ATOMIC_ROOT="$base/system" \
      ZXS_ATOMIC_LOG="$temporary/events" ZXS_ATOMIC_MODE=create \
      ZXS_ATOMIC_DESTINATION="$destination" ZXS_ATOMIC_READ_DESTINATION=1 \
      ZXS_ATOMIC_TRIGGER="$base/.inject" ZXS_ATOMIC_RECOVERY_TRIGGER="$base/.recover" \
      ZXS_ACTIVATION_IO_MODE="$mode" ZXS_ATOMIC_STAGE="$stage" \
      "$binary" --ignored --exact --nocapture --test-threads=1 --color never "$test_name" | tee "$temporary/result"
    # libtest may print the completion marker on its `test ...` progress line.
    if ! grep -Eq "(^| [.][.][.] )ZXS_ACTIVATION_IO_COMPLETE mode=$mode stage=$stage$" "$temporary/result" \
        || ! grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$temporary/result"; then
      echo "Missing control I/O completion or single-test pass for $mode:$stage" >&2
      exit 1
    fi
    rm -f "$temporary/events"
  done
done
