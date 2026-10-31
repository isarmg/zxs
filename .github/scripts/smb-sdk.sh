#!/usr/bin/env bash
# Exact Debian 13 source/configuration and apt runtime, not an upstream lookalike host.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
[[ "$(. /etc/os-release; printf '%s:%s' "$ID" "$VERSION_ID")" == debian:13 ]]
[[ "$(dpkg --print-architecture)" == amd64 ]]
version="$(python3 -c 'import json;print(json.load(open(".github/samba-debian-target.json"))["package_version"])')"
for package in samba samba-common-bin samba-dev smbclient; do
  [[ "$(dpkg-query -W -f='${Version}' "$package")" == "$version" ]]
done
cache="$root/target/smb-debian13"
mkdir -p "$cache/source"
if [[ ! -f "$cache/source/.downloaded" ]]; then
  (cd "$cache/source"; apt-get source --download-only --only-source "samba=$version")
  python3 - "$cache/source" <<'PYVERIFY'
import hashlib, json, pathlib, sys
base = pathlib.Path(sys.argv[1])
for name, expected in json.load(open('.github/samba-debian-target.json'))['source_files'].items():
    with (base / name).open('rb') as source:
        if hashlib.file_digest(source, 'sha256').hexdigest() != expected:
            raise ValueError('Debian source checksum mismatch: ' + name)
PYVERIFY
  (cd "$cache/source"; dpkg-source -x samba_*.dsc sdk)
  touch "$cache/source/.downloaded"
fi
source="$cache/source/sdk"
recipe="$(sha256sum .github/scripts/smb-sdk.sh .github/samba-debian-target.json | sha256sum | cut -d' ' -f1)"
if [[ ! -f "$source/.zxs-sdk-ready" ]] || [[ "$(cat "$source/.zxs-sdk-ready")" != "$recipe" ]]; then
  (
    cd "$source"
    # Run the exact Debian configure recipe (profiling/cluster/layout flags
    # affect internal structures); only limit the compiled SDK target set.
    DEB_BUILD_OPTIONS=parallel=2 make -f debian/rules configure
    PYTHONHASHSEED=1 ./buildtools/bin/waf build --targets=smbd/smbd,client/smbclient -j2
    printf '%s\n' "$recipe" > .zxs-sdk-ready
  )
fi
export ZXS_SMB_SDK_SOURCE="$source"
export ZXS_SMB_SDK_BUILD="$source/bin/default"
export ZXS_SMB_SDK_CACHE="$cache"
export LD_LIBRARY_PATH="$source/bin/shared:$source/bin/shared/private"
bash .github/scripts/smb-vfs-check.sh
unset LD_LIBRARY_PATH
mkdir -p "$cache/product"
cp "${CARGO_TARGET_DIR:-$root/target/native}/release/libsmb_vfs.so" "$cache/product/zxs.so"
python3 - "$cache/product" <<'PYRECEIPT'
import hashlib, json, pathlib, subprocess, sys
p = pathlib.Path(sys.argv[1]); target = json.load(open('.github/samba-debian-target.json'))
with (p/'zxs.so').open('rb') as source: checksum=hashlib.file_digest(source,'sha256').hexdigest()
receipt={**target,'commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
 'tree':subprocess.check_output(['git','rev-parse','HEAD^{tree}'],text=True).strip(), 'module_sha256':checksum,
 'host_version':subprocess.check_output(['smbd','--version'],text=True).strip()}
(p/'VFS-BUILD.json').write_text(json.dumps(receipt,indent=2)+'\n')
PYRECEIPT
readelf -d "$cache/product/zxs.so" > "$cache/product/zxs.dynamic.txt"
ldd "$cache/product/zxs.so" > "$cache/product/zxs.ldd.txt"
! grep -Fq 'not found' "$cache/product/zxs.ldd.txt"
# Build the real-engine fixture once; runtime acceptance runs it without Cargo.
cargo test --locked --offline -p storage-server --lib --no-run --message-format=json > "$cache/fixture-build.jsonl"
python3 - "$cache" <<'PYFIXTURE'
import json, pathlib, sys
cache=pathlib.Path(sys.argv[1]); candidates=[]
for line in (cache/'fixture-build.jsonl').read_text().splitlines():
    item=json.loads(line)
    if item.get('reason')=='compiler-artifact' and item.get('executable') and item['target']['name']=='storage_server':
        candidates.append(item['executable'])
if len(candidates)!=1: raise ValueError('expected one real storage-server test binary')
(cache/'fixture-path.txt').write_text(candidates[0]+'\n')
PYFIXTURE
