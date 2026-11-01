#!/usr/bin/env bash
# Isolated actual AuthStore consumer: no production filesystem or credential changes.
set -euo pipefail
binary="$(realpath "${1:?pass the storage-server libtest binary}")"
test_name=auth::tests::credential_io_barriers_preserve_publication_order_and_failure_fencing
listed="$("$binary" --list --ignored --exact "$test_name")"
[[ "$(grep -Fxc "$test_name: test" <<< "$listed")" == 1 ]] || { echo "Missing exact ignored auth I/O test" >&2; exit 1; }
fixture="$(cd "$(dirname "$0")/../../../platform/unit/fs_safety" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/atomic_io.c" -ldl -o "$temporary/atomic-io.so"
case "${2:-barriers}" in
  barriers) stages=(success write file-sync publish directory-sync) ;;
  rename) stages=(rename-after rename-before) ;;
  *) echo "unknown auth fault suite" >&2; exit 1 ;;
esac
for mode in plain backup management; do
  for stage in "${stages[@]}"; do
    root="$temporary/$mode-$stage"
    mkdir -m 700 "$root"
    destination=auth.json
    [[ "$mode" != management ]] || destination=management-auth.json
    echo "Auth I/O mode=$mode stage=$stage"
    env LD_PRELOAD="$temporary/atomic-io.so" ZXS_ATOMIC_ROOT="$root" \
      ZXS_ATOMIC_LOG="$temporary/events" ZXS_ATOMIC_MODE=replace \
      ZXS_ATOMIC_DESTINATION="$destination" ZXS_AUTH_MODE="$mode" \
      ZXS_ATOMIC_STAGE="$stage" "$binary" --ignored --exact --nocapture \
      --test-threads=1 --color never "$test_name" | tee "$temporary/result"
    if ! grep -Fqx "ZXS_AUTH_IO_COMPLETE mode=$mode stage=$stage" "$temporary/result" \
        || ! grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$temporary/result"; then
      echo "Missing auth I/O completion or single-test pass for $mode:$stage" >&2
      exit 1
    fi
    rm -f "$temporary/events"
  done
done
