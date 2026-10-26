#!/usr/bin/env bash
# Real Samba ABI validation. Never substitute hand-written/mock SDK structures.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
for tool in cc pkg-config python3 perl yapp flex bison m4 cargo curl tar sha256sum; do
  command -v "$tool" >/dev/null || {
    printf 'Missing SDK prerequisite: %s (see the smb-sdk CI job packages)\n' "$tool" >&2
    exit 1
  }
done
cache="${ZXS_SMB_SDK_CACHE:-$root/target/smb-sdk}"
mkdir -p "$cache"
cache="$(cd "$cache" && pwd)"
version=4.23.0
archive="$cache/samba-$version.tar.gz"
source="$cache/samba-$version"
sha256=b0cb963a63eb1515227f3779a46d3a7f790c1bbfeeb4b31fd43e405eefe7a3af
if [[ ! -f "$archive" ]]; then
  curl --fail --location --retry 3 --proto '=https' --tlsv1.2 \
    "https://download.samba.org/pub/samba/stable/samba-$version.tar.gz" -o "$archive.part"
  mv "$archive.part" "$archive"
fi
printf '%s  %s\n' "$sha256" "$archive" | sha256sum --check --strict
if [[ ! -d "$source" ]]; then
  tar -xzf "$archive" -C "$cache"
fi
# CI caches the complete configured source, generated NDR headers, and real libs.
# Reconfigure on script changes (also in the cache key), never reuse a partial build.
recipe="$(sha256sum "${BASH_SOURCE[0]}" | cut -d' ' -f1)"
if [[ ! -f "$source/.zxs-sdk-ready" ]] || [[ "$(cat "$source/.zxs-sdk-ready")" != "$recipe" ]]; then
  (
    cd "$source"
    ./configure --prefix="$cache/install" \
      --without-ad-dc --without-ads --without-ldap --without-pam \
      --disable-python --without-systemd --without-systemd-userdb \
      --without-json --without-libarchive --without-acl-support \
      --disable-cups --disable-iprint --disable-avahi --disable-glusterfs \
      --disable-cephfs --disable-spotlight --disable-wsp --without-regedit --without-gpgme \
      --without-ldb-lmdb --with-system-mitkrb5 \
      --bundled-libraries=talloc,tevent,tdb,ldb \
      --with-shared-modules='!vfs_snapper'
    # Building the real daemon/client generates dependent headers and libraries.
    # No install, daemon startup, account or host security configuration changes.
    PYTHONHASHSEED=1 ./buildtools/bin/waf build --targets=smbd/smbd,client/smbclient -j "${ZXS_SMB_SDK_JOBS:-2}"
    printf '%s\n' "$recipe" > .zxs-sdk-ready
  )
fi
export ZXS_SMB_SDK_SOURCE="$source"
export ZXS_SMB_SDK_BUILD="$source/bin/default"
export ZXS_SMB_SDK_CLANG_ARGS_JSON="${ZXS_SMB_SDK_CLANG_ARGS_JSON:-[]}"
# The SDK's shared libraries use their own sibling shared/private dependencies.
export LD_LIBRARY_PATH="$source/bin/shared:$source/bin/shared/private${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
python3 .github/scripts/check-rust-diagnostics.py -- \
  cargo clippy --locked --offline --message-format=json \
  -p smb-vfs --all-targets -- -D warnings
# Test harness output is not Cargo JSON. Gate compilation first, then run the
# already compiled tests without filtering their normal stdout/stderr.
python3 .github/scripts/check-rust-diagnostics.py -- \
  cargo test --locked --offline -p smb-vfs --no-run --message-format=json
cargo test --locked --offline -p smb-vfs
# Exercise the actual release crate with the configured SDK, not the test
# harness (which selects unwind independently). Reject only our expected gate.
abort_log="$cache/vfs-panic-abort.log"
if cargo rustc --locked --offline -p smb-vfs --release --lib -- -C panic=abort >"$abort_log" 2>&1; then
  cat "$abort_log"
  printf 'SMB VFS unexpectedly accepted panic=abort\n' >&2
  exit 1
fi
cat "$abort_log"
grep -Fq 'ZXS SMB VFS requires panic=unwind to isolate panics at the host boundary' "$abort_log"
# Finish with the supported release build; an unrelated negative build failure
# cannot pass the expected diagnostic gate or replace this positive check.
python3 .github/scripts/check-rust-diagnostics.py -- \
  cargo build --locked --offline -p smb-vfs --release --message-format=json
