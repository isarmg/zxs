#!/usr/bin/env bash
# Exact successful replay fsync must not ACK changed decision/metadata/FULL bytes.
set -euo pipefail
binary="$(realpath "${1:?pass the current storage-engine libtest binary}")"
directory="$(cd "$(dirname "$0")" && pwd)"
test_name=disk_authority::journal_tests::published_replay_rejects_same_inode_changes_and_lost_final_chain
listed="$("$binary" --list --ignored --exact "$test_name")"
[[ "$(grep -Fxc "$test_name: test" <<< "$listed")" == 1 ]]
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$directory/replay_mutation.c" -ldl -o "$temporary/replay-mutation.so"
for method in execute resume; do
    for kind in decision manifest body missing-body rollback; do
        case_root="$temporary/$method-$kind"
        mkdir -m 700 "$case_root"
        env LD_PRELOAD="$temporary/replay-mutation.so" ZXS_AUTHORITY_TEST_DISK="$case_root/disk" \
            ZXS_AUTHORITY_MUTATION_TRIGGER="$case_root/trigger" \
            ZXS_AUTHORITY_MUTATION_SELECTION="$case_root/selection" \
            ZXS_AUTHORITY_MUTATION_LOG="$case_root/events" \
            ZXS_AUTHORITY_MUTATION_KIND="$kind" ZXS_AUTHORITY_REPLAY_METHOD="$method" \
            "$binary" --ignored --exact --nocapture --test-threads=1 "$test_name" | tee "$case_root/result"
        [[ "$(grep -oF ZXS_INDEPENDENT_REPLAY_MUTATION_REJECTED "$case_root/result" | wc -l)" == 1 ]]
        [[ "$(grep -Ec '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in .+s$' "$case_root/result")" == 1 ]]
        printf 'Verified %s rejects %s changed during the successful replay barrier.\n' "$method" "$kind"
    done
done
