#!/usr/bin/env bash
# Temporary-directory first-probe evidence. No real devices or deployment data.
set -euo pipefail
binary="$(realpath "${1:?pass the storage-server libtest binary}")"
test_name=setup_recovery::activation::tests::activation_completion_never_probes_stopped_or_uncertain_initial_roots
listed="$("$binary" --list --ignored --exact "$test_name")"
[[ "$(grep -Fxc "$test_name: test" <<< "$listed")" == 1 ]] || {
  echo "Missing exact ignored activation first-probe test" >&2
  exit 1
}
source="$(cd "$(dirname "$0")" && pwd)/activation_probe_io.c"
temporary="$(mktemp -d "$PWD/.activation-probe-io.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$source" -ldl -o "$temporary/probe.so"
run() {
  local action="$1" data_only="$2" receipt="$3" locator="$4"
  local base="$temporary/$action-$data_only-$receipt-$locator"
  mkdir -m 700 "$base"
  env LD_PRELOAD="$temporary/probe.so" ZXS_ACTIVATION_PROBE_BASE="$base" \
    ZXS_ACTIVATION_PROBE_TARGET="$base/disks/one" \
    ZXS_ACTIVATION_PROBE_TRIGGER="$base/observe" \
    ZXS_ACTIVATION_PROBE_LOG="$base/events" \
    ZXS_ACTIVATION_PROBE_ACTION="$action" ZXS_ACTIVATION_PROBE_DATA_ONLY="$data_only" \
    ZXS_ACTIVATION_PROBE_RECEIPT="$receipt" ZXS_ACTIVATION_PROBE_LOCATOR="$locator" \
    "$binary" --ignored --exact --nocapture --test-threads=1 --color never "$test_name" \
    | tee "$base/result"
  grep -Fq "ZXS_ACTIVATION_PROBE_COMPLETE action=$action receipt=$receipt locator=$locator" "$base/result"
  grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$base/result"
}
run initial 0 missing intact
run initial 1 missing intact
run add 0 missing intact
for action in offline deregister; do
  for data_only in 0 1; do
    for receipt in missing corrupt mismatched; do
      run "$action" "$data_only" "$receipt" intact
    done
  done
done
for locator in missing head-missing history-missing head-corrupt dangling-record; do
  run initial 0 missing "$locator"
  run offline 1 missing "$locator"
done
