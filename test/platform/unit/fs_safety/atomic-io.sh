#!/usr/bin/env bash
# Run the already-built platform-io libtest in isolated children, never production.
set -euo pipefail
binary="$(realpath "${1:?pass the platform-io libtest binary}")"
fixture="$(cd "$(dirname "$0")" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/atomic_io.c" -ldl -o "$temporary/atomic-io.so"
for mode in create replace public; do
  for stage in success write file-sync publish directory-sync; do
    [[ "$mode:$stage" != public:write ]] || continue
    root="$temporary/$mode-$stage"
    mkdir -m 700 "$root"
    echo "Atomic I/O mode=$mode stage=$stage"
    env LD_PRELOAD="$temporary/atomic-io.so" ZXS_ATOMIC_ROOT="$root" \
      ZXS_ATOMIC_LOG="$temporary/events" ZXS_ATOMIC_MODE="$mode" \
      ZXS_ATOMIC_STAGE="$stage" "$binary" --ignored --exact --nocapture \
      fs_safety::tests::atomic_io_barriers_and_failures
    rm -f "$temporary/events"
  done
done
