#!/usr/bin/env bash
# Execute only exact, already-built libtest children. Run in GitHub with Rust 1.99.
set -euo pipefail
binary="$(realpath "${1:?pass the storage-engine libtest binary}")"
fixture="$(cd "$(dirname "$0")" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
prefix=disk::owned::directory_tests
for name in directory_sync_retry_in_isolated_child directory_sync_first_failure_in_isolated_child directory_sync_reopen_in_isolated_child; do
  listed="$("$binary" --list --ignored --exact "$prefix::$name")"
  if [[ "$(grep -Fxc "$prefix::$name: test" <<< "$listed")" != 1 ]]; then
    echo "Missing exact directory I/O regression: $prefix::$name" >&2
    exit 1
  fi
done
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/observe.c" -ldl -o "$temporary/observe.so"
run_child() {
  local root="$1" name="$2" marker="$3"
  if ! env LD_PRELOAD="$temporary/observe.so" ZXS_DIRECTORY_IO_ROOT="$root" \
    "$binary" --ignored --exact --nocapture --test-threads=1 --color never \
    "$prefix::$name" | tee "$temporary/result"; then
    return 1
  fi
  if [[ ! -f "$root/completed" || "$(cat "$root/completed")" != "$marker" ]] \
      || ! grep -Fqx "ZXS_DIRECTORY_IO_COMPLETE $marker" "$temporary/result" \
      || ! grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$temporary/result"; then
    echo "Missing directory I/O completion marker or single-test pass: $name" >&2
    exit 1
  fi
}
mkdir -m 700 "$temporary/same-owner" "$temporary/new-process"
status=0
run_child "$temporary/same-owner" directory_sync_retry_in_isolated_child retry-and-reuse || status=1
if run_child "$temporary/new-process" directory_sync_first_failure_in_isolated_child first-failure; then
  run_child "$temporary/new-process" directory_sync_reopen_in_isolated_child retry-and-reuse || status=1
else
  status=1
fi
if [[ "$status" != 0 ]]; then
  exit "$status"
fi
echo 'ZXS_DIRECTORY_IO_MATRIX_COMPLETE same-owner new-process'
