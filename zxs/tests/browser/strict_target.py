"""Run strict Clippy on the storage package's WASM target; never stage assets."""

from pathlib import Path
import subprocess
import sys
import tempfile

SOURCE = Path(__file__).resolve().parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(SOURCE))
from build_browser import TARGET, browser_environment, checked_browser_source


def lint(bindir, std):
    with tempfile.TemporaryDirectory(prefix="storage-browser-clippy-") as tmp, checked_browser_source(SOURCE):
        env, _, _, _ = browser_environment(SOURCE, Path(tmp), bindir, std)
        subprocess.run([bindir / "cargo", "clippy", "--manifest-path", SOURCE / "Cargo.toml",
                        "--target", TARGET, "--lib", "--locked", "--offline", "--",
                        "-D", "warnings", "-D", "clippy::pedantic"], cwd=SOURCE, env=env, check=True)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: strict_target.py REVIEWED_TOOLCHAIN_BIN VERIFIED_WASM_STD_ARCHIVE")
    lint(Path(sys.argv[1]).resolve(strict=True), Path(sys.argv[2]))
