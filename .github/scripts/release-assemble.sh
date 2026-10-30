#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${RELEASE_VERSION:?}"
v="$RELEASE_VERSION"
# Reject unexpected entries, including hidden files, directories and symlinks,
# before writing metadata or checksums into the release set.
python3 - "$v" <<'PYVALIDATE'
from pathlib import Path
import re
import stat
import sys
import runpy
import subprocess

version = sys.argv[1]
if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?", version):
    raise SystemExit("RELEASE_VERSION must be a safe version filename")
assets = Path("release-assets")
if not stat.S_ISDIR(assets.lstat().st_mode):
    raise SystemExit("release-assets must be a real directory")
expected = {f"zxs-{version}-x86_64-unknown-linux-gnu.tar.gz", f"web-interface-{version}.tar.gz", "BUILD.txt"}
if {path.name for path in assets.iterdir()} != expected:
    raise SystemExit("Release directory must contain exactly the two archives and native build receipt")
for name in expected:
    info = (assets / name).lstat()
    if not stat.S_ISREG(info.st_mode) or not info.st_size:
        raise SystemExit(f"Release archive must be a nonempty regular file: {name}")
validator = runpy.run_path('.github/scripts/verify-release-assets.py')
commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
validator['build_identity']((assets / 'BUILD.txt').read_bytes(), version, commit)
PYVALIDATE
{
  printf '# ZXS %s 安装\n\n' "$v"
  cat docs/templates/release-install.md
} > release-assets/INSTALL.md
# Preserve the receipt from the build runner; never relabel it with this job's
# checkout/toolchain. The native archive digest is checked against that receipt.
(cd release-assets && sha256sum -- * > SHA256SUMS && sha256sum --check SHA256SUMS)
python3 .github/scripts/verify-release-assets.py --version "$v" --commit "$(git rev-parse HEAD)"
