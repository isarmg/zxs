#!/usr/bin/env bash
# Observe only exact existing objects inside isolated libtest children.
set -euo pipefail
binary="$(realpath "${1:?pass the storage-engine libtest binary}")"
fixture="$(cd "$(dirname "$0")" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/observe.c" -ldl -o "$temporary/observe.so"
for stage in success file directory; do
  for member in 0 1 2; do
    [[ "$stage:$member" != success:1 && "$stage:$member" != success:2 ]] || continue
    root="$temporary/$stage-$member"
    mkdir -m 700 "$root"
    echo "Existing history object stage=$stage member=$member"
    env LD_PRELOAD="$temporary/observe.so" ZXS_HISTORY_IO_ROOT="$root" \
      ZXS_HISTORY_IO_LOG="$root/events" ZXS_HISTORY_IO_STAGE="$stage" \
      ZXS_HISTORY_IO_MEMBER="$member" "$binary" --ignored --exact --nocapture \
      --test-threads=1 history_package::prepared_dedup_tests::existing_object_syncs_in_isolated_child
    if [[ ! -f "$root/completed" ]]; then
      echo "Missing history I/O completion marker for $stage:$member" >&2
      exit 1
    fi
    if [[ "$(cat "$root/completed")" != "$stage:$member" ]]; then
      echo "Wrong history I/O completion marker for $stage:$member" >&2
      exit 1
    fi
  done
done
