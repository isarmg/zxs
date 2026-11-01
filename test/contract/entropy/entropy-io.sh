#!/usr/bin/env bash
# Isolated libc entropy failure; never changes host security or a running service.
set -euo pipefail
binary="$(realpath "${1:?pass a freshly built libtest binary}")"
test_name="${2:?pass the exact ignored entropy test name}"
listed="$("$binary" --list --ignored --exact "$test_name")"
[[ "$(grep -Fxc "$test_name: test" <<< "$listed")" == 1 ]] || { echo "Missing exact ignored entropy test" >&2; exit 1; }
fixture="$(cd "$(dirname "$0")" && pwd)"
temporary="$(mktemp -d)"
trap 'status=$?; if (( status == 0 )); then rm -rf "$temporary"; else echo "Entropy fault evidence retained: $temporary" >&2; fi' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/getrandom.c" -ldl -o "$temporary/entropy.so"
env LD_PRELOAD="$temporary/entropy.so" ZXS_TEST_ENTROPY_FAIL_FILE="$temporary/fail" \
  "$binary" --ignored --exact --nocapture --test-threads=1 --color never "$test_name" | tee "$temporary/result"
if ! grep -Fqx 'ZXS_ENTROPY_FAULT_COMPLETE' "$temporary/result" \
    || ! grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$temporary/result"; then
  echo 'Missing entropy completion marker or single-test pass' >&2
  exit 1
fi
