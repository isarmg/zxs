#!/usr/bin/env bash
# Validate actual source archives and four assets without compiling products.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/.github/scripts" "$fixture/compliance/licenses/upstream/sample" \
  "$fixture/compliance/provenance" "$fixture/docs/components"
cp .github/scripts/release-assemble.sh "$fixture/.github/scripts/"
printf 'complete legal notice\n' > "$fixture/compliance/NOTICE.md"
printf 'copyright fixture\n' > "$fixture/compliance/licenses/upstream/sample/LICENSE"
printf '{"upstream":"sample"}\n' > "$fixture/compliance/provenance/origins.json"
python3 - "$fixture" <<'PYFIXTURE'
from pathlib import Path
import sys
root = Path(sys.argv[1])
for index in range(4100):
    (root / "compliance/provenance" / f"record-{index}.json").write_text("{}\n")
PYFIXTURE
python3 test/ci/web_dist_fixture.py "$fixture"
native="$fixture/target/native/x86_64-unknown-linux-gnu/release"
printf 'test-only fixture\n' > "$native/browser-smoke"
printf 'unshipped tool\n' > "$native/zxs-unpublished-tool"
(umask 077; RELEASE_VERSION=0.1.0 bash "$fixture/.github/scripts/release-package.sh")
RELEASE_VERSION=0.1.0 bash "$fixture/.github/scripts/release-assemble.sh"
python3 - "$fixture" <<'PY'
import hashlib
import io
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile

root = Path(sys.argv[1])
assets = root / "release-assets"
expected = {"zxs-0.1.0-debian13-amd64", "zxs-helper-0.1.0-debian13-amd64",
            "zxs-source-0.1.0.tar.gz", "SHA256SUMS"}
assert {path.name for path in assets.iterdir()} == expected
with tarfile.open(assets / "zxs-source-0.1.0.tar.gz") as bundle:
    assert set(bundle.getnames()) == {"BUILD.txt", "VFS-BUILD.json", "INSTALL.md", "repository.tar.gz"}
    for entry in bundle:
        assert entry.isfile() and entry.mode == 0o644 and entry.uid == entry.gid == entry.mtime == 0
    installation = bundle.extractfile("INSTALL.md").read().decode()
    assert installation == "# ZXS 0.1.0 安装\n\n" + (root / "docs/templates/release-install.md").read_text()
    with tarfile.open(fileobj=io.BytesIO(bundle.extractfile("repository.tar.gz").read())) as source:
        names = set(source.getnames())
        assert "SOURCE-COMMIT" in names
        assert not any(name.startswith(("source/target/", "source/build/", "source/.git/")) for name in names)
        for entry in source:
            assert entry.isfile() and entry.mode in (0o644, 0o755)
            assert entry.uid == entry.gid == entry.mtime == 0
        for path in ("compliance/NOTICE.md", "compliance/licenses/upstream/sample/LICENSE",
                     "compliance/provenance/origins.json", "docs/templates/release-install.md",
                     "deploy/systemd/zxs.service", "deploy/samba/smb.conf.example",
                     "Cargo.toml", "Cargo.lock", "tools/web-build/build-inputs.json"):
            assert source.extractfile("source/" + path).read() == (root / path).read_bytes()
        assert sum(name.startswith("source/compliance/") for name in names) > 4096
for name in ("zxs", "zxs-helper"):
    assert (assets / f"{name}-0.1.0-debian13-amd64").read_bytes() == (root / "target/native/x86_64-unknown-linux-gnu/release" / name).read_bytes()

commands = re.search(r"```sh\n(.*?)\n```", installation, re.S).group(1)
def check(version="0.1.0"):
    script = commands.replace("export VERSION='已核实版本'", f"export VERSION='{version}'")
    return subprocess.run(["bash", "--noprofile", "--norc", "-c", script], cwd=assets,
                          env={"PATH": os.defpath, "LC_ALL": "C"}, capture_output=True, timeout=30)

source = assets / "zxs-source-0.1.0.tar.gz"
saved_source = source.read_bytes()
source.write_bytes(saved_source + b"damaged download\n")
assert check().returncode != 0
source.write_bytes(saved_source)
receipt = assets / "SHA256SUMS"
original_receipt = receipt.read_text()
lines = original_receipt.splitlines(keepends=True)
bad_receipts = ["".join(lines[:index] + lines[index + 1:]) for index in range(len(lines))]
bad_receipts += ["", original_receipt + lines[0], original_receipt + "0" * 64 + "  unexpected.tar.gz\n",
                 original_receipt.replace("  zxs-source-", "  ../zxs-source-"),
                 original_receipt.replace("  zxs-source-", " *zxs-source-")]
for bad in bad_receipts:
    receipt.write_text(bad)
    assert check().returncode != 0, "incomplete, duplicate or unsafe checksum inventory must fail"
receipt.write_text(original_receipt)
for name in expected:
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
            assert check().returncode != 0, f"{kind} input must fail: {name}"
            if kind == "directory":
                path.rmdir()
            elif kind != "missing":
                path.unlink()
    finally:
        saved.rename(path)
for bad in ("", "../escape", "0.1.0/../../escape", "0.1", "0.1.0\nother", "$(touch bad)"):
    assert check(bad).returncode != 0
result = check()
assert result.returncode == 0, result.stderr.decode()
assert {path.name for path in assets.iterdir()} == expected, "download check must not install or extract"
# Existing output is never replaced, even after an input failure.
originals = {path.name: path.read_bytes() for path in assets.iterdir()}
retry = subprocess.run(["bash", str(root / ".github/scripts/release-package.sh")],
                       env=dict(os.environ, RELEASE_VERSION="0.1.0"), capture_output=True)
assert retry.returncode != 0
assert {path.name: path.read_bytes() for path in assets.iterdir()} == originals
PY
echo 'PASS four assets retain exact source, legal material and receipts without installing products'

