#!/usr/bin/env bash
# Invoke the compiled storage-server libtest binary in isolated fault children.
set -euo pipefail
binary="$(realpath "${1:?pass the storage-server libtest binary}")"
test_name="smb_rpc::budget::tests::real_budget_io_failure_preserves_source_charge_and_error"
if ! "$binary" --ignored --exact --list "$test_name" | grep -Fx "$test_name: test"; then
  echo "Missing exact ignored SMB budget I/O test" >&2
  exit 1
fi
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
      "$test_name" | tee "$temporary/$grow-$stage.log"
    if ! grep -Fq "test result: ok. 1 passed; 0 failed; 0 ignored;" "$temporary/$grow-$stage.log"; then
      echo "Missing single-test SMB budget I/O pass" >&2
      exit 1
    fi
  done
done
