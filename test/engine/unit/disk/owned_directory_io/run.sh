#!/usr/bin/env bash
# Execute only exact, already-built libtest children. Run in GitHub with Rust 1.99.
set -euo pipefail
binary="$(realpath "${1:?pass the storage-engine libtest binary}")"
fixture="$(cd "$(dirname "$0")" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
prefix=disk::owned::directory_tests
for name in directory_sync_retry_in_isolated_child directory_sync_first_failure_in_isolated_child directory_sync_reopen_in_isolated_child directory_sync_replacement_in_isolated_child directory_sync_parallel_in_isolated_child directory_sync_eviction_in_isolated_child directory_sync_engine_recovery_in_isolated_child; do
  listed="$("$binary" --list --ignored --exact "$prefix::$name")"
  if [[ "$(grep -Fxc "$prefix::$name: test" <<< "$listed")" != 1 ]]; then
    echo "Missing exact directory I/O regression: $prefix::$name" >&2
    exit 1
  fi
done
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/observe.c" -ldl -o "$temporary/observe.so"
run_child() {
  local root="$1" name="$2" marker="$3" failure="${4:-parent}"
  local status=0
  env LD_PRELOAD="$temporary/observe.so" ZXS_DIRECTORY_IO_ROOT="$root" \
    ZXS_DIRECTORY_IO_FAILURE="$failure" \
    "$binary" --ignored --exact --nocapture --test-threads=1 --color never \
    "$prefix::$name" | tee "$temporary/result" || status=1
  echo "Directory I/O events after $name (libc fsync only):"
  if [[ -f "$root/events" ]]; then
    cat "$root/events"
  fi
  if [[ "$status" != 0 ]]; then
    return 1
  fi
  if [[ ! -f "$root/completed" || "$(cat "$root/completed")" != "$marker" ]] \
      || ! grep -Fqx "ZXS_DIRECTORY_IO_COMPLETE $marker" "$temporary/result" \
      || ! grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$temporary/result"; then
    echo "Missing directory I/O completion marker or single-test pass: $name" >&2
    exit 1
  fi
}
status=0
for failure in child parent; do
  same="$temporary/same-owner-$failure"
  reopened="$temporary/new-process-$failure"
  mkdir -m 700 "$same" "$reopened"
  run_child "$same" directory_sync_retry_in_isolated_child retry-and-reuse "$failure" || status=1
  if run_child "$reopened" directory_sync_first_failure_in_isolated_child first-failure "$failure"; then
    run_child "$reopened" directory_sync_reopen_in_isolated_child retry-and-reuse "$failure" || status=1
  else
    status=1
  fi
done
for mode in replacement parallel eviction; do
  root="$temporary/$mode"
  mkdir -m 700 "$root"
  marker=retry-and-reuse
  [[ "$mode" != parallel ]] || marker=parallel
  run_child "$root" "directory_sync_${mode}_in_isolated_child" "$marker" || status=1
done
root="$temporary/engine-recovery"
mkdir -m 700 "$root"
run_child "$root" directory_sync_engine_recovery_in_isolated_child engine-recovery || status=1
if [[ "$status" != 0 ]]; then
  exit "$status"
fi
echo 'ZXS_DIRECTORY_IO_MATRIX_COMPLETE child parent same-owner new-process replacement parallel eviction engine-recovery'
