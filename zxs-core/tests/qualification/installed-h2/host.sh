#!/usr/bin/env bash
# Product/CA changes occur only inside the no-NIC guest booted below.
set -euo pipefail
umask 077
test "${GITHUB_ACTIONS:?}" = true
test "${GITHUB_REPOSITORY:?}" = isarmg/zxs
test "${GITHUB_EVENT_NAME:?}" = workflow_dispatch
test "${GITHUB_REF:?}" = refs/heads/main
test "${GITHUB_WORKFLOW_REF:?}" = isarmg/zxs/.github/workflows/zxs-core-installed-h2.yml@refs/heads/main
test "$(id -u)" -ne 0
[[ "${GITHUB_SHA:?}" =~ ^[0-9a-f]{40}$ ]]
test "$(git rev-parse HEAD)" = "$GITHUB_SHA"
harness="$PWD/zxs-core/tests/qualification/installed-h2"
work="$(mktemp -d -- "${RUNNER_TEMP:?}/zxs-core-installed-h2.XXXXXXXX")"
cd "$work"
mkdir old new service rust home cargo old-target new-target helper-target tmp seed
old=1bc616dc07001b0a0b2d807ca756a078fbdb3904
new=b2958a6d1fbcdb6d61c31a79fa1c648e6b0fdbe5
service=5c7815f1bb9241b2bbea3f59daf512694661e8fc
git_env=(env -i PATH=/usr/bin:/bin LC_ALL=C HOME="$work/home" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0)
"${git_env[@]}" git init --quiet --bare source.git
for sha in "$old" "$new" "$service"; do
    "${git_env[@]}" git --git-dir=source.git fetch --quiet --no-tags --depth=1 \
        https://github.com/isarmg/zxs.git "$sha"
    test "$(git --git-dir=source.git rev-parse FETCH_HEAD)" = "$sha"
done
git --git-dir=source.git archive "$old" zxs-core | tar -xf - -C old --no-same-owner --no-same-permissions
git --git-dir=source.git archive "$new" zxs-core | tar -xf - -C new --no-same-owner --no-same-permissions
git --git-dir=source.git archive "$service" zxs helper | tar -xf - -C service --no-same-owner --no-same-permissions
cat <<'PINS' | sha256sum -c -
5f646a89312e230181634fc82ec7e5ce82f2b42db134f49b19e9c9c56f53079a  old/zxs-core/Cargo.lock
0f556d1419f7443de085ea92d5642f1c9b59674b80714cc2513b85fe4596393b  new/zxs-core/Cargo.lock
2b898fbfa5a396d4291c7b9c7e7b1439e5c8434835e24711622324c8b4f05fe4  service/zxs/Cargo.lock
9dea3e14264ad491146cc648e01184a152209aa262ab3970ef441b0bfa36475d  service/helper/Cargo.lock
PINS
printf 'qualify client old=%s installed=%s server/helper=%s harness=%s\n' "$old" "$new" "$service" "$GITHUB_SHA"

fetch() {
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 15 --max-time 240 --retry 2 --max-filesize 600000000 "$1" -o "$2"
}
arch=x86_64-unknown-linux-gnu
while read -r checksum component member; do
    filename="$component-1.100.0-$arch"
    fetch "https://static.rust-lang.org/dist/2026-11-12/$filename.tar.xz" "$filename.tar.xz"
    printf '%s  %s.tar.xz\n' "$checksum" "$filename" | sha256sum -c -
    tar -xJf "$filename.tar.xz" -C rust --strip-components=2 --no-same-owner --no-same-permissions \
        --exclude="$filename/$member/manifest.in" "$filename/$member"
done <<'RUST'
f1d0b1dec81dab6f523cd098507763487b0d4bd7fd2c87582f154128719be03b rustc rustc
7e29ce07d7c6e92d1826870575ba27d57fff6cb3e754aa57d99099cb88fab35d cargo cargo
4591b6486594ded52ecd6135db95bf9355bbd3ca7a380888b141528bbcf9a5d4 rust-std rust-std-x86_64-unknown-linux-gnu
RUST
bin="$work/rust/bin"
test "$("$bin/rustc" --version --verbose | sed -n 's/^commit-hash: //p')" = 0ef1bc386059596dada8fb65b50425aab2ea637a
build=(env -i PATH="$bin:/usr/bin:/bin" HOME="$work/home" TMPDIR="$work/tmp" LC_ALL=C TZ=UTC
    CARGO_HOME="$work/cargo" CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 RUSTC="$bin/rustc"
    RUSTDOC="$bin/rustdoc" CC=/usr/bin/gcc AR=/usr/bin/ar
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=/usr/bin/gcc)
sha256sum /usr/bin/gcc /usr/bin/ar /usr/bin/ld
for label in old new; do
    src="$work/$label/zxs-core"
    (cd "$src"; "${build[@]}" "$bin/cargo" fetch --locked --target "$arch")
    "${build[@]}" CARGO_TARGET_DIR="$work/$label-target" "$bin/cargo" install --locked --offline \
        --target "$arch" --path "$src" --root "$work/install-$label" --bin zxs-client
    install -m 0555 "install-$label/bin/zxs-client" "seed/zxs-client-$label"
done
if cmp -s seed/zxs-client-old seed/zxs-client-new; then exit 1; fi
(cd service/zxs; python3 -I -B build_release.py "$bin")
install -m 0555 service/zxs/target/standalone/zxs seed/zxs
(cd service/helper
 "${build[@]}" "$bin/cargo" fetch --locked --target "$arch"
 "${build[@]}" CARGO_TARGET_DIR="$work/helper-target" "$bin/cargo" build --locked --offline --release --target "$arch" --bin helper)
install -m 0555 "helper-target/$arch/release/helper" seed/helper
for exe in seed/zxs-client-old seed/zxs-client-new seed/zxs seed/helper; do
    size=$(stat -c %s "$exe")
    (( size >= 64 && size <= 67108864 ))
    readelf -h "$exe" | grep -qE '^ *Machine: +Advanced Micro Devices X86-64$'
done
sha256sum seed/zxs-client-old seed/zxs-client-new seed/zxs seed/helper

# Preserve the host's system install and trust. The signed Ubuntu guest and
# boot tool DEBs are only fetched and extracted into disposable runner scratch.
image=ubuntu-26.04-minimal-cloudimg-amd64.img
release=https://cloud-images.ubuntu.com/minimal/releases/resolute/release-20261118
digest=f6c630c04d4fad012f2c4ed3a21a2b0c0f814f13f6873568da4c31d8df65c12e
fetch "$release/SHA256SUMS" SHA256SUMS
fetch "$release/SHA256SUMS.gpg" SHA256SUMS.gpg
mkdir gnupg
GNUPGHOME="$work/gnupg" gpgv --keyring /usr/share/keyrings/ubuntu-cloudimage-keyring.gpg \
    --status-fd 1 SHA256SUMS.gpg SHA256SUMS > image.signature
grep -qF '[GNUPG:] VALIDSIG D2EB44626FDDC30B513D5BB71A5D6C4C7DB87C81 ' image.signature
grep -qxF "$digest *$image" SHA256SUMS
fetch "$release/$image" "$image"
printf '%s *%s\n' "$digest" "$image" | sha256sum -c -

mkdir -p apt/{etc/empty,lists/partial,archives/partial} tools
: > apt/status
export APT_CONFIG="$work/apt/config"
cat > "$APT_CONFIG" <<EOF
Dir::Etc "$work/apt/etc";
Dir::Etc::main "/dev/null";
Dir::Etc::parts "empty";
Dir::Etc::sourceparts "empty";
Dir::Etc::sourcelist "sources.list";
Dir::State "$work/apt";
Dir::State::status "$work/apt/status";
Dir::Cache "$work/apt";
Dir::Log "$work/apt";
Dir::Cache::pkgcache "";
Dir::Cache::srcpkgcache "";
Acquire::Languages "none";
Acquire::Retries "2";
APT::Get::Always-Include-Phased-Updates "true";
APT::Update::Error-Mode "any";
EOF
snapshot=https://snapshot.ubuntu.com/ubuntu/20261118T120000Z
archive_key=/etc/apt/trusted.gpg.d/ubuntu-keyring-2018-archive.gpg
for suite in noble noble-updates noble-security; do
    printf 'deb [arch=amd64 signed-by=%s] %s %s main universe\n' "$archive_key" "$snapshot" "$suite"
done > apt/etc/sources.list
/usr/bin/apt-get update
(cd apt/lists; cat <<'PINS' | sha256sum -c -
cdb2f31d809f589719a53c6ad15f255b27569c4059542ada282aaa21b8e164b0  snapshot.ubuntu.com_ubuntu_20261118T120000Z_dists_noble_InRelease
a4dfe9c58d77db89e8b45240d1cc14a3e6dd9159bc29395aebfe6e893594dccb  snapshot.ubuntu.com_ubuntu_20261118T120000Z_dists_noble-updates_InRelease
65a6f9c5cf5b8d887e45c2a638b19bedd86e9d3630afee8b3a7b46ea5803dc22  snapshot.ubuntu.com_ubuntu_20261118T120000Z_dists_noble-security_InRelease
PINS
)
/usr/bin/apt-get --assume-yes --download-only --no-install-recommends install qemu-system-x86 ovmf genisoimage e2fsprogs
for deb in apt/archives/*.deb; do dpkg-deb --extract "$deb" tools; done
sha256sum apt/archives/*.deb
unset APT_CONFIG

# Pure wheel harness runtime only, not a product client dependency.
python3 -I -B -m venv h2venv
mkdir wheels py-deps
pip=(h2venv/bin/python -I -B -m pip --isolated --disable-pip-version-check --no-input)
locked=(--no-deps --only-binary=:all: --require-hashes --no-cache-dir -r "$harness/h2-requirements.txt")
"${pip[@]}" download --index-url https://pypi.org/simple --dest wheels "${locked[@]}"
test "$(find wheels -maxdepth 1 -type f -name '*-py3-none-any.whl' | wc -l)" -eq 3
"${pip[@]}" install --no-index --find-links wheels --target py-deps "${locked[@]}"
test -z "$(find py-deps ! -type f ! -type d -print -quit)"
tar -cf seed/py-deps.tar -C py-deps .
install -m 0444 "$harness/guest-setup.sh" "$harness/guest.py" "$harness/h2_relay.py" seed/

# Synthetic short-lived CA keys remain inside runner scratch; only leaf keys
# and public roots go into the guest. No OpenSSL configuration is installed.
make_ca() {
    openssl req -new -newkey rsa:2048 -nodes -x509 -days 2 -sha256 \
        -subj "/CN=ZXS isolated $1 fixture root" -addext 'basicConstraints=critical,CA:TRUE' \
        -addext 'keyUsage=critical,keyCertSign,cRLSign' -keyout "$1.key" -out "seed/$1.crt" 2>>cert-build.log
}
make_leaf() {
    local name=$1 ip=$2
    openssl req -new -newkey rsa:2048 -nodes -subj "/CN=$ip" -keyout "seed/$name.key" \
        -out "$name.csr" 2>>cert-build.log
    printf '%s\n' 'basicConstraints=critical,CA:FALSE' 'keyUsage=critical,digitalSignature,keyEncipherment' \
        'extendedKeyUsage=serverAuth' "subjectAltName=IP:$ip" > "$name.ext"
    openssl x509 -req -days 1 -sha256 -CA seed/issuing.crt -CAkey issuing.key -CAcreateserial \
        -CAserial issuing.serial -in "$name.csr" -extfile "$name.ext" -out "seed/$name.pem" 2>>cert-build.log
    openssl verify -purpose sslserver -verify_ip "$ip" -CAfile seed/issuing.crt "seed/$name.pem"
}
make_ca issuing
make_ca unrelated
make_leaf server 127.0.0.1
make_leaf wrong-san 127.0.0.2
(cd seed
 sha256sum zxs helper zxs-client-old zxs-client-new guest-setup.sh guest.py h2_relay.py py-deps.tar \
     issuing.crt unrelated.crt server.pem server.key wrong-san.pem wrong-san.key > SHA256SUMS)
printf '%s\n' 'instance-id: zxs-core-installed-h2-ca-v1' > seed/meta-data
cat > seed/user-data <<'CLOUD'
#cloud-config
users: []
ssh_pwauth: false
ssh_genkeytypes: []
package_update: false
package_upgrade: false
runcmd:
  - - /bin/bash
    - -ec
    - |
      exec > /dev/ttyS0 2>&1
      udevadm settle --timeout=15
      mkdir -m 0755 /mnt/zxs-core-qualification-seed
      mount -o ro,nosuid,nodev,noexec /dev/disk/by-id/virtio-zxs-core-seed /mnt/zxs-core-qualification-seed
      /bin/bash /mnt/zxs-core-qualification-seed/guest-setup.sh
power_state:
  mode: poweroff
  timeout: 60
  condition: true
CLOUD

prefix="$work/tools"
libs="$prefix/usr/lib/x86_64-linux-gnu:$prefix/lib/x86_64-linux-gnu"
loader="$prefix/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2"
tool=(env -i LANG=C PATH=/usr/bin:/bin HOME="$work/home" TMPDIR="$work/tmp"
    QEMU_MODULE_DIR="$prefix/usr/lib/x86_64-linux-gnu/qemu"
    MKE2FS_CONFIG="$prefix/etc/mke2fs.conf" "$loader" --inhibit-cache --library-path "$libs")
for exe in usr/sbin/mke2fs usr/bin/genisoimage usr/bin/qemu-system-x86_64; do
    "${tool[@]}" --list "$prefix/$exe" | awk -v base="$prefix/" '$2 == "=>" && index($3, base) != 1 {exit 1}'
done
mkdir empty
disks=()
for role in data-a data-b recycle client; do
    truncate -s 64M "$role.ext4"
    "${tool[@]}" "$prefix/usr/sbin/mke2fs" -q -t ext4 -d empty -L "zxs-$role" "$role.ext4"
    disks+=(-drive "if=none,id=$role,format=raw,file=$role.ext4" -device "virtio-blk-pci,drive=$role,serial=zxs-core-$role")
done
"${tool[@]}" "$prefix/usr/bin/genisoimage" -quiet -volid cidata -joliet -rock -output seed.iso seed
firmware="$prefix/usr/share/OVMF"
cp "$firmware/OVMF_VARS_4M.fd" vars.fd
status=0
timeout -k 15s 12m "${tool[@]}" "$prefix/usr/bin/qemu-system-x86_64" \
    -no-user-config -nodefaults -machine q35 -accel tcg,thread=multi -smp 2 -m 2048 \
    -nic none -display none -monitor none -serial file:serial.log -no-reboot -L "$prefix/usr/share/qemu" \
    -drive "if=pflash,format=raw,readonly=on,file=$firmware/OVMF_CODE_4M.fd" \
    -drive if=pflash,format=raw,file=vars.fd -drive "if=virtio,format=qcow2,file=$image" \
    -drive if=none,id=seed,format=raw,readonly=on,file=seed.iso \
    -device virtio-blk-pci,drive=seed,serial=zxs-core-seed "${disks[@]}" || status=$?
test -s serial.log
tr -d '\r' < serial.log | grep -aE '^ZXS-CORE-INSTALLED-H2 ' > qualified.log || :
cat qualified.log
test "$status" -eq 0
test "$(grep -Fxc 'ZXS-CORE-INSTALLED-H2 old installs committed two original replies intentionally withheld' qualified.log)" -eq 1
test "$(grep -Fxc 'ZXS-CORE-INSTALLED-H2 fresh unrelated-root receipt and Basic each read exact original only' qualified.log)" -eq 1
test "$(grep -Fxc 'ZXS-CORE-INSTALLED-H2 cold verified readback and all pre-credential refusals passed' qualified.log)" -eq 1
test "$(grep -Fxc 'ZXS-CORE-INSTALLED-H2 no-NIC guest trust mutations verified and fixture completed' qualified.log)" -eq 1
test "$(wc -l < qualified.log)" -eq 4

# Only validated bounds and synthetic outcomes are persisted in the run summary.
{
    printf '### Verified isolated installed clients (Linux, ext4)\n\n'
    printf 'Old client: `%s`; candidate: `%s`; ZXS + Helper: `%s`; harness: `%s`.\n\n' \
        "$old" "$new" "$service" "$GITHUB_SHA"
    printf '%s\n' \
        '- Two durable original saves, one intentionally lost success reply per old install.' \
        '- Receipt-only recovery: one empty login, one exact original GET, zero resource writes.' \
        '- Ordinary Basic recovery: one exact original GET, zero bootstrap or resource writes.' \
        '- Exact profile retained and later edit still dirty, with one independent installed whole-node readback.' \
        '- Decoded native CA set matched both old enrollments; exactly the synthetic unrelated root was added, then exactly the issuer was removed.' \
        '- Fresh missing-issuer, wrong-SAN and successful TLS without H2: no prompt or HTTP authorization; mismatched profile endpoints: no TCP.' \
        '- CA changes were limited to the no-NIC guest. No released package, GUI, Range or resume claim.'
} >> "${GITHUB_STEP_SUMMARY:?}"
