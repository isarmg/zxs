"""Small complete release fixture; no compiler or executable product required."""
import hashlib
import json
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[2]
SPEC = "tools/web-build/build-inputs.json"


def record(root):
    spec = json.loads((root / SPEC).read_text())
    inputs = set(spec["files"])
    for directory in spec["directories"]:
        inputs.update(path.relative_to(root).as_posix() for path in (root / directory).rglob("*") if path.is_file())
    dist = root / "build/web"
    manifest = {
        "runtime": "web-interface", "target": "wasm32-unknown-unknown", "wasm_bindgen": "0.2.129",
        "inputs": {path: hashlib.sha256((root / path).read_bytes()).hexdigest() for path in sorted(inputs)},
        "files": {path.relative_to(dist).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
                  for path in sorted(dist.rglob("*")) if path.is_file() and path.name != "build-manifest.json"},
    }
    (dist / "build-manifest.json").write_text(json.dumps(manifest))
    return manifest


def prepare(root):
    root = Path(root)
    spec = json.loads((ROOT / SPEC).read_text())
    for directory in spec["directories"]:
        (root / directory).mkdir(parents=True, exist_ok=True)
    for relative in [*spec["files"], "README.md", "compliance/NOTICE.md", "docs/README.md", "deploy/systemd/zxs.service",
                     "deploy/samba/smb.conf.example",
                     "target/native/release/zxs", "target/native/release/zxs-rescue", "build/web/index.html", "build/web/boot.js",
                     "build/web/zxs.css", "build/web/styles/startup.css", "build/web/pkg/zxs.js",
                     "build/web/pkg/zxs_bg.wasm", "build/web/pkg/zxs_bg.wasm.gz"]:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            path.write_text(f"fixture {relative}\n")
    for name in ("zxs", "zxs-rescue"):
        (root / "target/native/release" / name).chmod(0o755)
    for relative in (SPEC, ".github/scripts/release-package.sh", ".github/scripts/verify-web-dist.py"):
        shutil.copy2(ROOT / relative, root / relative)
    return record(root)


if __name__ == "__main__":
    prepare(Path(sys.argv[1]))
