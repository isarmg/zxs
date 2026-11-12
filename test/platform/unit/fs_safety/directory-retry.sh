#!/usr/bin/env bash
set -euo pipefail
binary="$(realpath "${1:?pass the platform-io or storage-engine libtest binary}")"
component="${2:?pass platform or engine}"
case "$component" in
  platform) test_name=fs_safety::tests::directory_creation_retry_barriers; modes='create child trusted';;
  engine) test_name=disk::owned::directory_retry_tests::directory_creation_retry_barriers; modes='engine engine-path';;
  *) exit 2;;
esac
listed="$("$binary" --list --ignored --exact "$test_name")"
[[ "$(grep -Fxc "$test_name: test" <<< "$listed")" == 1 ]]
fixture="$(cd "$(dirname "$0")" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror "$fixture/directory_retry.c" -ldl -o "$temporary/directory-retry.so"
for mode in $modes; do
  for stage in child parent; do
    root="$temporary/$mode-$stage"
    mkdir -m 700 "$root"
    env LD_PRELOAD="$temporary/directory-retry.so" ZXS_DIRECTORY_ROOT="$root" \
      ZXS_DIRECTORY_LOG="$temporary/events" ZXS_DIRECTORY_MODE="$mode" \
      ZXS_DIRECTORY_STAGE="$stage" "$binary" --ignored --exact --nocapture \
      --test-threads=1 --color never "$test_name" | tee "$temporary/result"
    grep -Fqx "ZXS_DIRECTORY_RETRY_COMPLETE mode=$mode stage=$stage" "$temporary/result"
    grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$temporary/result"
    rm "$temporary/events"
  done
done

if [[ "$component" == engine ]]; then
  create_test=disk::owned::working_directory::tests::
  create_test+=private_entry_sync_failure_preserves_created_identity_and_held_directory
  install_test=disk::owned::working_directory::tests::
  install_test+=working_directory_rename_sync_failure_retains_installed_outcome
  for selected_test in "$create_test" "$install_test"; do
    listed="$("$binary" --list --ignored --exact "$selected_test")"
    [[ "$(grep -Fxc "$selected_test: test" <<< "$listed")" == 1 ]]
  done
  for stage in child parent; do
    root="$temporary/working-create-$stage"
    mkdir -m 700 "$root"
    env LD_PRELOAD="$temporary/directory-retry.so" ZXS_DIRECTORY_ROOT="$root" \
      ZXS_DIRECTORY_LOG="$temporary/events" ZXS_DIRECTORY_MODE=working-create \
      ZXS_DIRECTORY_STAGE="$stage" "$binary" --ignored --exact --nocapture \
      --test-threads=1 --color never "$create_test" | tee "$temporary/result"
    grep -Fqx "ZXS_WORKING_DIRECTORY_COMPLETE operation=create stage=$stage" \
      "$temporary/result"
    grep -Eq \
      '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' \
      "$temporary/result"
    rm "$temporary/events"
  done
  # This host is created by the test after exclusive creation of all three roots.
  # Precreating common-data would correctly fail the create-only fixture.
  root="$temporary/working-install/common-data/host"
  env LD_PRELOAD="$temporary/directory-retry.so" ZXS_DIRECTORY_ROOT="$root" \
    ZXS_DIRECTORY_LOG="$temporary/events" ZXS_DIRECTORY_MODE=working-install \
    ZXS_DIRECTORY_STAGE=parent "$binary" --ignored --exact --nocapture \
    --test-threads=1 --color never "$install_test" | tee "$temporary/result"
  grep -Fqx "ZXS_WORKING_DIRECTORY_COMPLETE operation=install stage=parent" \
    "$temporary/result"
  grep -Eq \
    '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' \
    "$temporary/result"
  rm "$temporary/events"
fi
