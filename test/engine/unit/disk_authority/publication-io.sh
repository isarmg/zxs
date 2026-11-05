#!/usr/bin/env bash
# Inject actual std File fsync EIO into an isolated test child, never a real disk.
set -euo pipefail
binary="$(realpath "${1:?pass the current storage-engine libtest binary}")"
root="$(cd "$(dirname "$0")/../../.." && pwd)"
test_name=disk_authority::journal_tests::published_replay_repeats_the_real_file_and_directory_barriers
listed="$("$binary" --list --ignored --exact "$test_name")"
[[ "$(grep -Fxc "$test_name: test" <<< "$listed")" == 1 ]]
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$root/platform/unit/fs_safety/atomic_io.c" -ldl -o "$temporary/atomic-io.so"
env LD_PRELOAD="$temporary/atomic-io.so" ZXS_AUTHORITY_TEST_DISK="$temporary/disk" \
    ZXS_ATOMIC_ROOT="$temporary/disk/publications" ZXS_ATOMIC_DESTINATION=00000000000000000001.json \
    ZXS_ATOMIC_TRIGGER="$temporary/inject" ZXS_ATOMIC_RECOVERY_TRIGGER="$temporary/recovery" \
    ZXS_ATOMIC_LOG="$temporary/events" ZXS_ATOMIC_READ_DESTINATION=1 ZXS_ATOMIC_STAGE=directory-sync \
    "$binary" --ignored --exact --nocapture --test-threads=1 "$test_name" | tee "$temporary/result"
[[ "$(grep -oF ZXS_INDEPENDENT_DURABILITY_REPLAY_COMPLETE "$temporary/result" | wc -l)" == 1 ]]
[[ "$(grep -Ec '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in .+s$' "$temporary/result")" == 1 ]]
bash "$(dirname "$0")/replay-mutation-io.sh" "$binary"
