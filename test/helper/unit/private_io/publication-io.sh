#!/usr/bin/env bash
# Isolated libc-backend tests only; no ptrace or host policy changes.
set -euo pipefail
binary="$(realpath "${1:?pass the current storage-helper bin-test executable}")"
cli="$(realpath "${2:?pass the current zxs-helper executable}")"
test_name=private_io::tests::publication_faults_preserve_phase_identity_and_evidence
listed="$("$binary" --list --ignored --exact "$test_name")"
[[ "$(grep -Fxc "$test_name: test" <<< "$listed")" == 1 ]] || { echo "missing exact helper fault test" >&2; exit 1; }
fixture="$(cd "$(dirname "$0")/../../../platform/unit/fs_safety" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
cc -shared -fPIC -Wall -Wextra -Werror '-DZXS_STAGING_PREFIX=".zxs-helper-"' \
  "$fixture/atomic_io.c" -ldl -o "$temporary/publication.so"
for mode in create replace; do
  for stage in success file-sync directory-sync rename-before rename-after create-race; do
    [[ "$mode:$stage" != replace:create-race ]] || continue
    root="$temporary/$mode-$stage"
    mkdir "$root"
    env LD_PRELOAD="$temporary/publication.so" ZXS_ATOMIC_ROOT="$root" \
      ZXS_ATOMIC_LOG="$temporary/events" ZXS_ATOMIC_MODE="$mode" ZXS_ATOMIC_STAGE="$stage" \
      "$binary" --ignored --exact --nocapture --test-threads=1 --color never "$test_name" | tee "$temporary/result"
    grep -Fqx "ZXS_RESCUE_PUBLICATION_COMPLETE mode=$mode stage=$stage" "$temporary/result"
    grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;' "$temporary/result"
    rm "$temporary/events"
  done
done
# Exercise the actual application boundary: a failed backup cannot mutate draft.
for stage in rename-before rename-after; do
  root="$temporary/cli-$stage"
  mkdir "$root"
  "$cli" configuration template --disks "$root/disk" --output "$root/draft.json" > "$temporary/template"
  cp "$root/draft.json" "$temporary/original"
  touch "$root/.inject"
  if env LD_PRELOAD="$temporary/publication.so" ZXS_ATOMIC_ROOT="$root" \
    ZXS_ATOMIC_LOG="$temporary/events" ZXS_ATOMIC_MODE=create ZXS_ATOMIC_STAGE="$stage" \
    ZXS_ATOMIC_DESTINATION=draft.json.before-registration \
    "$cli" configuration register-disk --config "$root/draft.json" --slot P02 \
    --disk-id disk_00000000000000000000000000000001 --path "$root/second" --level 1 \
    > "$temporary/stdout" 2> "$temporary/stderr"; then
    echo "injected backup failure returned success" >&2; exit 1
  fi
  cmp "$root/draft.json" "$temporary/original"
  grep -Fq 'cannot preserve configuration backup' "$temporary/stderr"
  grep -Fq 'the draft was not changed' "$temporary/stderr"
  grep -Fq 'atomic publication outcome is unknown' "$temporary/stderr"
  grep -Fq 'Input/output error (os error 5)' "$temporary/stderr"
  grep -Fq 'staging' "$temporary/stderr"
  test ! -s "$temporary/stdout"
  if [[ "$stage" == rename-after ]]; then
    cmp "$root/draft.json.before-registration" "$temporary/original"
    grep -Fxq 'rename-success-reported-eio' "$temporary/events"
  else
    test ! -e "$root/draft.json.before-registration"
    grep -Fxq 'rename-before-eio' "$temporary/events"
    staged=("$root"/.zxs-helper-*.tmp)
    [[ "${#staged[@]}" == 1 ]]
    cmp "${staged[0]}" "$temporary/original"
  fi
  echo "ZXS_RESCUE_CLI_BACKUP_COMPLETE stage=$stage"
  rm "$temporary/events"
done
