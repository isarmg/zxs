#!/usr/bin/env bash
# Run the already-built platform-io libtest in isolated children, never production.
set -euo pipefail
binary="$(realpath "${1:?pass the platform-io libtest binary}")"
test_name=fs_safety::tests::atomic_io_barriers_and_failures
listed="$("$binary" --list --ignored --exact "$test_name")"
if [[ "$(grep -Fxc "$test_name: test" <<< "$listed")" != 1 ]]; then
  echo "Missing exact ignored atomic I/O test: $test_name" >&2
  exit 1
fi
fixture="$(cd "$(dirname "$0")" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/atomic_io.c" -ldl -o "$temporary/atomic-io.so"
for mode in create replace replace-held public; do
  for stage in success write file-sync publish directory-sync; do
    [[ "$mode:$stage" != public:write ]] || continue
    root="$temporary/$mode-$stage"
    mkdir -m 700 "$root"
    echo "Atomic I/O mode=$mode stage=$stage"
    env LD_PRELOAD="$temporary/atomic-io.so" ZXS_ATOMIC_ROOT="$root" \
      ZXS_ATOMIC_LOG="$temporary/events" ZXS_ATOMIC_MODE="$mode" \
      ZXS_ATOMIC_STAGE="$stage" "$binary" --ignored --exact --nocapture \
      --test-threads=1 --color never "$test_name" | tee "$temporary/result"
    if ! grep -Fqx "ZXS_ATOMIC_IO_COMPLETE mode=$mode stage=$stage" "$temporary/result" \
        || ! grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$temporary/result"; then
      echo "Missing atomic I/O completion or single-test pass for $mode:$stage" >&2
      exit 1
    fi
    rm -f "$temporary/events"
  done
done
