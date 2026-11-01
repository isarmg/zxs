#!/usr/bin/env bash
# Run only on Linux hosts already permitting strace; never alter ptrace policy.
set -euo pipefail
binary=$(realpath "${1:?pass the freshly built storage-engine unit-test executable}")
output=$(realpath -m "${2:?pass an output directory for fault evidence}")
source_dir=$(cd -- "$(dirname -- "$0")" && pwd)
seal_test=engine::observations::tests::seal_permission_failure_preserves_source_and_revokes_epoch
capacity_test=engine::observations::tests::capacity_syscall_failure_preserves_source_without_revoking_metadata_epoch
mkdir -p "$output"
for name in "$seal_test" "$capacity_test"; do
  listed=$("$binary" --ignored --exact "$name" --list)
  printf '%s\n' "$listed"
  [[ $(grep -Fxc "$name: test" <<< "$listed") == 1 ]]
done
cc -shared -fPIC -Wall -Wextra -Werror "$source_dir/read_error.c" -ldl -o "$output/read-error.so"
env LD_PRELOAD="$output/read-error.so" "$binary" --ignored --exact "$seal_test" \
  --nocapture --test-threads=1 2>&1 | tee "$output/seal-permission.log"
[[ $(grep -Fc ZXS_OBSERVATION_SEAL_EACCES "$output/seal-permission.log") == 1 ]]
[[ $(grep -Ec '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$output/seal-permission.log") == 1 ]]
strace -f -e trace=fstatfs -e inject=fstatfs:error=EIO -o "$output/capacity.strace" \
  "$binary" --ignored --exact "$capacity_test" \
  --nocapture --test-threads=1 2>&1 | tee "$output/capacity.log"
cat "$output/capacity.strace"
[[ $(grep -Ec 'fstatfs.*EIO.*INJECTED' "$output/capacity.strace") == 2 ]]
[[ $(grep -Ec '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$output/capacity.log") == 1 ]]
