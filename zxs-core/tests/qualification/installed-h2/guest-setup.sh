#!/usr/bin/env bash
# Cloud-init calls this only from the original read-only miniature seed in QEMU.
set -euo pipefail
umask 077
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C.UTF-8
unset SSL_CERT_FILE SSL_CERT_DIR

media=/mnt/zxs-core-qualification-seed
dest=/opt/zxs-core-qualification
marks=/run/zxs-core-qualification-control
marker=/run/zxs-core-qualification-fixture
state=/zxs/state/headless
bundle=/etc/ssl/certs/ca-certificates.crt
anchor=/usr/local/share/ca-certificates/zxs-core-fixture-issuing.crt
unrelated=/usr/local/share/ca-certificates/zxs-core-fixture-unrelated.crt
child=
stage=admission

fail() { printf 'ZXS-CORE-INSTALLED-H2 guest failed %s\n' "$stage" >&2; exit 1; }
[[ "$(id -u)" == 0 ]] || fail
[[ "$(systemd-detect-virt --vm)" == qemu ]] || fail
[[ "$(ls -1 /sys/class/net)" == lo ]] || fail
for ns in pid user mnt net; do
    [[ "$(stat -Lc '%d:%i' "/proc/thread-self/ns/$ns")" == "$(stat -Lc '%d:%i' "/proc/1/ns/$ns")" ]] || fail
done
[[ "$(stat -Lc '%d:%i' /)" == "$(stat -Lc '%d:%i' /proc/1/root)" ]] || fail
[[ "$(realpath -- "$0")" == "$media/guest-setup.sh" ]] || fail
[[ "$(findmnt -nro FSTYPE --mountpoint "$media")" == iso9660 ]] || fail
[[ ",$(findmnt -nro OPTIONS --mountpoint "$media")," == *,ro,* ]] || fail
test -b /dev/disk/by-id/virtio-zxs-core-seed
[[ "$(realpath -- /dev/disk/by-id/virtio-zxs-core-seed)" == "$(findmnt -nro SOURCE --mountpoint "$media")" ]] || fail
for f in /zxs "$dest" "$marker" "$marks" "$anchor" "$unrelated" /var/lib/zxs-acceptance-system; do
    test ! -e "$f" && test ! -L "$f" || fail
done
[[ -z "$(getent passwd 23100 || :)" && -z "$(getent group 23100 || :)" ]] || fail
command -v openssl >/dev/null
command -v update-ca-certificates >/dev/null

cleanup() {
    local rc=$?
    if [[ -n "$child" ]]; then
        kill "$child" 2>/dev/null || :
        wait "$child" 2>/dev/null || :
    fi
    # Only remove verified synthetic inputs installed by this guest recipe.
    local cert original changed=false
    for cert in "$anchor" "$unrelated"; do
        if [[ "$cert" == "$anchor" ]]; then original="$media/issuing.crt"
        else original="$media/unrelated.crt"; fi
        if [[ -f "$cert" && ! -L "$cert" ]] && cmp -s "$original" "$cert"; then
            rm -- "$cert"
            changed=true
        fi
    done
    if [[ "$changed" == true ]]; then update-ca-certificates --fresh >/dev/null 2>&1 || rc=1; fi
    if (( rc != 0 )); then printf 'ZXS-CORE-INSTALLED-H2 guest failed %s\n' "$stage" >&2; fi
    exit "$rc"
}
trap cleanup EXIT

stage=verify-seed
cd "$media"
sha256sum --strict -c SHA256SUMS
install -m 0755 -d "$dest" "$dest/old/bin" "$dest/new/bin" "$dest/deps" "$dest/fixture-tls" /zxs /zxs/bin
install -m 0755 zxs helper /zxs/bin/
install -m 0755 zxs-client-old "$dest/old/bin/zxs-client"
install -m 0755 zxs-client-new "$dest/new/bin/zxs-client"
install -m 0444 guest.py h2_relay.py "$dest/"
install -m 0444 server.pem wrong-san.pem "$dest/fixture-tls/"
install -m 0400 -o 23100 -g 23100 server.key wrong-san.key "$dest/fixture-tls/"
python3 -I -B - "$dest/deps" <<'PY'
from pathlib import Path
import sys,tarfile
with tarfile.open("py-deps.tar", "r:") as source:
    source.extractall(sys.argv[1], filter="data")
for path in Path(sys.argv[1]).rglob("*"):
    assert not path.is_symlink() and (path.is_dir() or path.is_file()), path
    path.chmod(0o755 if path.is_dir() else 0o444)
PY

stage=mounts
install -d -m 0700 -o 23100 -g 23100 "$state" /var/lib/zxs-acceptance-system
for role in data-a data-b recycle client; do
    if [[ "$role" == client ]]; then target=/mnt/zxs-core-qualification-client
    else target="/mnt/zxs-acceptance-$role"; fi
    test ! -e "$target" && test ! -L "$target"
    install -m 0755 -d "$target"
    mount -t ext4 -o rw,nosuid,nodev,noexec "/dev/disk/by-id/virtio-zxs-core-$role" "$target"
    rmdir "$target/lost+found"
    chown 23100:23100 "$target"
    chmod 0700 "$target"
done
install -d -m 0755 "$marks" "$marks/ready"
install -d -m 0700 -o 23100 -g 23100 "$marks/request"
printf 'zxs-core-no-nic-qualification-v1\n' > "$marker"
chmod 0444 "$marker"

native_chain() {
    openssl verify -CAfile "$bundle" -no-CApath -no-CAstore -purpose sslserver "$@" >/dev/null 2>&1
}
store_root() {
    local file=$1 path=$2
    (set -o noclobber; cat -- "$file" > "$path")
    chmod 0644 "$path"
    update-ca-certificates --fresh >/dev/null 2>&1
}
wait_for_guest_phase() {
    local name=$1 path="$marks/request/$1" deadline=$(( SECONDS + 240 ))
    until [[ -d "$path" && ! -L "$path" ]]; do
        (( SECONDS < deadline )) && kill -0 "$child" || return 1
        sleep 0.1
    done
    [[ "$(stat -c '%u:%g:%a' "$path")" == 23100:23100:700 ]]
}
release_guest() {
    local pending="$marks/ready/.pending-$1"
    (set -o noclobber; printf 'ok\n' > "$pending")
    chmod 0444 "$pending"
    mv -T -- "$pending" "$marks/ready/$1"
}

stage=original-issuer
if native_chain -verify_ip 127.0.0.1 server.pem; then fail; fi
store_root issuing.crt "$anchor"
native_chain -verify_ip 127.0.0.1 server.pem
native_chain wrong-san.pem
if native_chain -verify_ip 127.0.0.1 wrong-san.pem; then fail; fi

stage=ordinary-fixture
/usr/bin/setpriv --reuid=23100 --regid=23100 --clear-groups --no-new-privs \
    /usr/bin/python3 -I -B -c '
import runpy, sys
base = "/opt/zxs-core-qualification"
sys.path[:0] = [base, base + "/deps"]
runpy.run_path(base + "/guest.py", run_name="__main__")
' &
child=$!

stage=unrelated-root
wait_for_guest_phase add-unrelated
store_root unrelated.crt "$unrelated"
native_chain -verify_ip 127.0.0.1 server.pem
release_guest add-unrelated

stage=removed-root
wait_for_guest_phase remove-issuing
test -f "$anchor" && test ! -L "$anchor" && cmp -s issuing.crt "$anchor"
rm -- "$anchor"
update-ca-certificates --fresh >/dev/null 2>&1
if native_chain -verify_ip 127.0.0.1 server.pem; then fail; fi
release_guest remove-issuing
stage=ordinary-evidence
result=0
wait "$child" || result=$?
child=
test "$result" -eq 0
printf '%s\n' 'ZXS-CORE-INSTALLED-H2 no-NIC guest trust mutations verified and fixture completed'
