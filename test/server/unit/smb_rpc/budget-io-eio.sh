#!/usr/bin/env bash
# Invoke the compiled storage-server libtest binary in isolated fault children.
set -euo pipefail
binary="$(realpath "${1:?pass the storage-server libtest binary}")"
fixture="$(cd "$(dirname "$0")" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/budget_io_eio.c" -ldl -o "$temporary/budget-io-eio.so"
for grow in 0 1; do
  for stage in write file-sync directory-sync; do
    root="$temporary/$grow-$stage"
    mkdir -m 700 "$root"
    echo "SMB budget grow=$grow stage=$stage"
    env LD_PRELOAD="$temporary/budget-io-eio.so" \
      ZXS_SMB_BUDGET_FAULT_ROOT="$root" \
      ZXS_SMB_BUDGET_FAULT_STAGE="$stage" ZXS_SMB_BUDGET_GROW="$grow" \
      "$binary" --ignored --exact --nocapture \
      smb_rpc::budget::tests::real_budget_io_failure_preserves_source_charge_and_error
  done
done

for test in \
  smb_rpc::budget::tests::real_cleanup_directory_sync_failure_keeps_charge_and_blocks_admission \
  smb_rpc::tests::cleanup_tests::real_close_directory_sync_failure_keeps_published_outcome_and_consumes_handle; do
  root="$temporary/${test##*::}"
  mkdir -m 700 "$root"
  env LD_PRELOAD="$temporary/budget-io-eio.so" \
    ZXS_SMB_BUDGET_FAULT_ROOT="$root" ZXS_SMB_BUDGET_FAULT_STAGE=directory-sync \
    "$binary" --ignored --exact --nocapture "$test"
done
