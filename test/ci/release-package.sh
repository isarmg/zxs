#!/usr/bin/env bash
# Validate real archive contents without compiling or publishing.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/.github/scripts" "$fixture/target/native/release" \
  "$fixture/build/web/pkg" "$fixture/compliance/licenses/upstream/sample" \
  "$fixture/compliance/provenance" "$fixture/docs/components" "$fixture/docs/templates"
cp .github/scripts/release-package.sh .github/scripts/release-assemble.sh "$fixture/.github/scripts/"
cp rust-toolchain.toml "$fixture/"
for name in zxs zxs-rescue; do
  printf '#!/bin/sh\nexit 0\n' > "$fixture/target/native/release/$name"
  chmod +x "$fixture/target/native/release/$name"
done
# Validation tools must never leak into either product archive.
printf 'test-only engine fixture\n' > "$fixture/target/native/release/browser-smoke"
printf 'unshipped tool\n' > "$fixture/target/native/release/zxs-unpublished-tool"
printf '<html>fixture</html>\n' > "$fixture/build/web/index.html"
printf '{"runtime":"web-interface"}\n' > "$fixture/build/web/build-manifest.json"
printf 'wasm fixture\n' > "$fixture/build/web/pkg/zxs_bg.wasm"
printf 'complete legal notice\n' > "$fixture/compliance/NOTICE.md"
printf 'copyright fixture\n' > "$fixture/compliance/licenses/upstream/sample/LICENSE"
printf '{"upstream":"sample"}\n' > "$fixture/compliance/provenance/origins.json"
for path in README.md docs/README.md docs/release.md docs/25-operation-manual.md docs/THIRD_PARTY_RUST_SOURCES.md; do
  printf 'documentation fixture\n' > "$fixture/$path"
done
printf '[SMB integration](components/smb.md)\n' >> "$fixture/docs/release.md"
printf '[Operations](../25-operation-manual.md)\n' > "$fixture/docs/components/smb.md"
printf '[Samba configuration](../../deploy/samba/smb.conf.example)\n' >> "$fixture/docs/components/smb.md"
cp docs/templates/release-install.md "$fixture/docs/templates/"
# Reproduce the real material size without putting thousands of files online.
python3 - "$fixture" <<'PYFIXTURE'
from pathlib import Path
import sys
root = Path(sys.argv[1])
for index in range(4100):
    (root / "compliance/provenance" / f"record-{index}.json").write_text("{}\n")
legal = root / "build/web/compliance"
legal.mkdir()
(legal / "NOTICE.md").write_bytes((root / "compliance/NOTICE.md").read_bytes())
PYFIXTURE
python3 test/ci/web_dist_fixture.py "$fixture"
(umask 077; RELEASE_VERSION=0.1.0 bash "$fixture/.github/scripts/release-package.sh")
for archive in "$fixture"/release-assets/*.tar.gz; do
  python3 - "$archive" <<'PYMODES'
import sys
import tarfile
with tarfile.open(sys.argv[1]) as archive:
    for entry in archive:
        assert entry.uid == entry.gid == 0, entry.name
        assert not entry.mode & 0o022, entry.name
        assert entry.mode & 0o044 == 0o044, entry.name
        if entry.isdir():
            assert entry.mode == 0o755, entry.name
PYMODES
  output=$(mktemp -d "$fixture/extract.XXXXXX")
  tar -xzf "$archive" -C "$output"
  test -z "$(find "$output" -name browser-smoke -print -quit)"
  test -z "$(find "$output" -name zxs-unpublished-tool -print -quit)"
  test ! -d "$output/test"
  test ! -d "$output/tools"
  for path in compliance/NOTICE.md compliance/licenses/upstream/sample/LICENSE \
    compliance/provenance/origins.json docs/25-operation-manual.md \
    docs/components/smb.md docs/templates/release-install.md deploy/systemd/zxs.service \
    deploy/samba/smb.conf.example; do
    cmp "$fixture/$path" "$output/$path"
  done
  test "$(stat -c %a "$output/deploy/systemd")" = 755
  test "$(stat -c %a "$output/deploy/systemd/zxs.service")" = 644
  test "$(stat -c %a "$output/deploy/samba")" = 755
  test "$(stat -c %a "$output/deploy/samba/smb.conf.example")" = 644
  python3 - "$output" <<'PY'
from pathlib import Path
import re
import sys

root = Path(sys.argv[1])
for document in (root / "docs/release.md", root / "docs/components/smb.md"):
    for relative in re.findall(r"\]\(([^)]+)\)", document.read_text()):
        target = (document.parent / relative).resolve()
        assert target.is_relative_to(root.resolve()) and target.is_file(), target
PY
  if [[ "$archive" == *-unknown-linux-gnu.tar.gz ]]; then
    for name in zxs zxs-rescue; do
      cmp "$fixture/target/native/release/$name" "$output/$name"
    done
  else
    cmp "$fixture/build/web/pkg/zxs_bg.wasm" "$output/public/pkg/zxs_bg.wasm"
    cmp "$fixture/build/web/build-manifest.json" "$output/public/build-manifest.json"
    cmp "$fixture/compliance/NOTICE.md" "$output/public/compliance/NOTICE.md"
    test ! -e "$output/public/docs"
    test ! -e "$output/public/compliance/provenance"
    test "$(find "$output/compliance" -type f | wc -l)" -gt 4096
    test "$(find "$output/public" -type f | wc -l)" -lt 4096
  fi
done
# Assemble the same two real archives with a fixed source identity, without
# creating a Git repository or contacting a publishing service.
mkdir "$fixture/bin"
cat > "$fixture/bin/git" <<'SH'
#!/bin/sh
test "$*" = 'rev-parse HEAD' || exit 2
printf '0123456789012345678901234567890123456789\n'
SH
chmod +x "$fixture/bin/git"
PATH="$fixture/bin:$PATH" RELEASE_VERSION=0.1.0 bash "$fixture/.github/scripts/release-assemble.sh"
test "$(head -1 "$fixture/release-assets/INSTALL.md")" = '# ZXS 0.1.0 安装'
tail -n +3 "$fixture/release-assets/INSTALL.md" | cmp "$fixture/docs/templates/release-install.md" -
python3 - "$fixture/release-assets" <<'PY'
from pathlib import Path
import hashlib
import os
import re
import subprocess
import sys

assets = Path(sys.argv[1])
commands = re.search(r"```sh\n(.*?)\n```", (assets / "INSTALL.md").read_text(), re.S).group(1)
server = assets / "server-0.1.0"
web = assets / "web-0.1.0"

def install():
    # No inherited errexit, shell startup file, aliases or exported functions.
    return subprocess.run(
        ["bash", "--noprofile", "--norc", "-c", commands], cwd=assets,
        env={"PATH": os.defpath, "LC_ALL": "C"}, capture_output=True, timeout=30,
    )

archive = assets / "web-interface-0.1.0.tar.gz"
original = archive.read_bytes()
archive.write_bytes(original + b"damaged download\n")
assert install().returncode != 0
assert not server.exists() and not web.exists(), "checksum failure must precede extraction"
archive.write_bytes(original)
receipt = assets / "SHA256SUMS"
original_receipt = receipt.read_text()
lines = original_receipt.splitlines(keepends=True)
bad_receipts = ["".join(lines[:index] + lines[index + 1:]) for index in range(len(lines))]
bad_receipts += [
    "",
    original_receipt + lines[0],
    original_receipt + "0" * 64 + "  unexpected.tar.gz\n",
    original_receipt.replace("  web-interface-0.1.0.tar.gz", "  ../web-interface-0.1.0.tar.gz"),
    original_receipt.replace("  INSTALL.md", " *INSTALL.md"),
]
for bad in bad_receipts:
    receipt.write_text(bad)
    assert install().returncode != 0, "incomplete, duplicate or unsafe checksum inventory must fail"
    assert not server.exists() and not web.exists(), "inventory failure must precede extraction"
receipt.write_text(original_receipt)
# Installation must reject links even when their target has the expected bytes,
# and must reject FIFOs before any checksum reader can block on opening one.
for name in ("BUILD.txt", "INSTALL.md", "SHA256SUMS",
             "web-interface-0.1.0.tar.gz", "zxs-0.1.0-x86_64-unknown-linux-gnu.tar.gz"):
    path = assets / name
    saved = assets / (name + ".fixture-original")
    path.rename(saved)
    try:
        for kind in ("missing", "symlink", "directory", "fifo", "empty"):
            if kind == "symlink":
                path.symlink_to(saved.name)
            elif kind == "directory":
                path.mkdir()
            elif kind == "fifo":
                os.mkfifo(path)
            elif kind == "empty":
                path.touch()
            result = install()
            assert result.returncode != 0, f"{kind} release input must fail: {name}"
            assert not server.exists() and not web.exists(), "invalid input must precede extraction"
            if kind == "directory":
                path.rmdir()
            elif kind != "missing":
                path.unlink()
    finally:
        saved.rename(path)
build = assets / "BUILD.txt"
original_build = build.read_text()

def refresh_checksums():
    names = ["BUILD.txt", "INSTALL.md", *sorted(path.name for path in assets.glob("*.tar.gz"))]
    receipt.write_text("".join(hashlib.sha256((assets / name).read_bytes()).hexdigest()
                               + "  " + name + "\n" for name in names))

for bad in ("", "../escape", "0.1.0/../../escape", "0.1", "0.1.0\nversion=0.1.0"):
    build.write_text(original_build.replace("version=0.1.0\n", "version=" + bad + "\n"))
    refresh_checksums()
    assert install().returncode != 0, "invalid or duplicate version must fail even with matching checksum"
    assert not server.exists() and not web.exists(), "version failure must precede extraction"
build.write_text(original_build)
receipt.write_text(original_receipt)
result = install()
assert result.returncode == 0, result.stderr.decode()
assert (server / "docs/components/smb.md").is_file()
assert (web / "public/pkg/zxs_bg.wasm").read_bytes() == b"wasm fixture\n"
# A second copy-and-paste must stop before extracting over existing files.
(server / "zxs").write_bytes(b"existing installation\n")
retry = install()
assert retry.returncode != 0
assert (server / "zxs").read_bytes() == b"existing installation\n"
# Preserve prerelease/build metadata accepted by the release filename grammar.
version = "0.1.0-rc.1+build.7"
for path in assets.glob("*.tar.gz"):
    path.rename(path.with_name(path.name.replace("0.1.0", version)))
build.write_text(original_build.replace("version=0.1.0\n", "version=" + version + "\n"))
refresh_checksums()
result = install()
assert result.returncode == 0, result.stderr.decode()
assert (assets / ("server-" + version) / "zxs").is_file()
assert (assets / ("web-" + version) / "public/pkg/zxs_bg.wasm").is_file()
PY
echo 'PASS release archives retain complete documentation and assembly emits standalone installation instructions'
