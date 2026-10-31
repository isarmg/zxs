#!/usr/bin/env bash
# Diagnose the actual supported browser launch before any expensive product build.
# This probe does not prove ZXS UI correctness and never disables the sandbox.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
[[ "$(id -u)" != 0 ]] || { echo 'browser probe requires non-root UID' >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p artifacts
{
  id
  dpkg-query -W -f='${Package} ${Version}\n' chromium chromium-driver chromium-sandbox
  stat -c '%a %u:%g %n' /usr/lib/chromium/chrome-sandbox
  printf 'user namespace limit: '; cat /proc/sys/user/max_user_namespaces
} > artifacts/debian13-browser-environment.log
# No proxy/network permissions, external content, real account or persistent profile.
timeout --kill-after=5s 30s /usr/bin/chromium --headless=new --disable-dev-shm-usage \
  --enable-logging=stderr --user-data-dir="$work/profile" --dump-dom about:blank \
  > artifacts/debian13-browser-probe.html 2> artifacts/debian13-browser-probe.log || {
    status=$?
    printf '%s\n' "$status" > artifacts/debian13-browser-exit-code.txt
    cat artifacts/debian13-browser-environment.log >&2
    cat artifacts/debian13-browser-probe.log >&2
    exit "$status"
  }
grep -q '<html' artifacts/debian13-browser-probe.html
printf '0\n' > artifacts/debian13-browser-exit-code.txt
