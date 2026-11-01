#!/usr/bin/env bash
set -euo pipefail
binary="$(realpath "${1:?pass the storage-server libtest binary}")"
test_name=telemetry::tests::enumeration_failure_preserves_source_and_uses_live_memory_history
"$binary" --ignored --exact --list "$test_name" | grep -Fx "$test_name: test"
fixture="$(cd "$(dirname "$0")" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/enumeration_eio.c" -ldl -o "$temporary/enumeration-eio.so"
mkdir -m 700 "$temporary/cache"
env LD_PRELOAD="$temporary/enumeration-eio.so" ZXS_TELEMETRY_ENUM_ROOT="$temporary/cache" \
  "$binary" --ignored --exact --nocapture --test-threads=1 "$test_name" 2>&1 | tee "$temporary/result"
[[ "$(grep -o 'ZXS_TELEMETRY_ENUM_EIO_AFTER_ENTRY' "$temporary/result" | wc -l)" == 2 ]]
grep -Fq 'monitoring cache unavailable; using bounded memory history' "$temporary/result"
grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored;' "$temporary/result"
