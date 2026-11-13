"""GitHub-only, two real Engine builds. Never substitute a decoder model for old code."""
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
OLD = "d8333f91b049a014f62c677d034b1b84f5232a92"
OLD_TREE = "de7dcab7d5c33d98f8cf3f1e9e8d4743a62659ce"
FIXTURES = ROOT / "test/ci/fixtures/storage-format-fence"
EXAMPLE = "zxs_actual_revision_format_fence"


def image(root):
    rows = {}
    for path in [root, *sorted(root.rglob("*"))]:
        info = path.lstat()
        if stat.S_ISREG(info.st_mode):
            value = ("file", path.read_bytes().hex())
        elif stat.S_ISDIR(info.st_mode):
            value = ("directory", "")
        elif stat.S_ISLNK(info.st_mode):
            value = ("symlink", os.readlink(path))
        else:
            raise AssertionError(f"unexpected fixture object: {path}")
        rows[str(path.relative_to(root))] = [info.st_dev, info.st_ino, info.st_mode, info.st_nlink, *value]
    return rows


class ActualStorageFormatFence(unittest.TestCase):
    def test_fixed_v2_and_current_v3_engines_refuse_without_writes(self):
        if os.environ.get("GITHUB_ACTIONS") != "true":
            self.skipTest("actual cross-revision builds execute only in explicit GitHub CI")
        started = time.monotonic()
        evidence = ROOT / "target/targeted-ci/wire/storage-format-fence"
        evidence.mkdir(mode=0o700, parents=True, exist_ok=False)
        records = []

        def run(command, cwd, expected=0, maximum=240):
            remaining = 840 - (time.monotonic() - started)
            self.assertGreater(remaining, 0, "fixed total fence budget exhausted")
            index = len(records)
            try:
                result = subprocess.run(command, cwd=cwd, text=True, stdout=subprocess.PIPE,
                                        stderr=subprocess.STDOUT, timeout=min(maximum, remaining), check=False)
            except subprocess.TimeoutExpired as error:
                output = error.stdout or b""
                if isinstance(output, bytes):
                    output = output.decode(errors="replace")
                (evidence / f"{index:02d}.log").write_text(output)
                records.append({"command": [str(item) for item in command], "cwd": str(cwd),
                                "timeout_seconds": min(maximum, remaining), "expected": expected})
                (evidence / "operations.json").write_text(json.dumps(records, indent=2))
                raise
            (evidence / f"{index:02d}.log").write_text(result.stdout)
            records.append({"command": [str(item) for item in command], "cwd": str(cwd),
                            "exit": result.returncode, "expected": expected})
            (evidence / "operations.json").write_text(json.dumps(records, indent=2))
            print(json.dumps(records[-1]), flush=True)
            self.assertEqual(result.returncode, expected, result.stdout[-12000:])
            return result.stdout.strip()

        with tempfile.TemporaryDirectory(prefix="zxs-real-format-fence-") as temporary:
            work = Path(temporary)
            old = work / "old-reader"
            current = work / "current-reader"
            head = run(["git", "rev-parse", "HEAD"], ROOT)
            run(["git", "fetch", "--no-tags", "https://github.com/isarmg/zxs.git", OLD], ROOT, maximum=90)
            self.assertEqual(run(["git", "rev-parse", f"{OLD}^{{tree}}"], ROOT), OLD_TREE)
            run(["git", "worktree", "add", "--detach", str(old), OLD], ROOT, maximum=90)
            run(["git", "worktree", "add", "--detach", str(current), head], ROOT, maximum=90)
            try:
                source = (FIXTURES / "reader.rs").read_text()
                binaries = {}
                for label, checkout in [("old", old), ("current", current)]:
                    expected_head = OLD if label == "old" else head
                    self.assertEqual(run(["git", "rev-parse", "HEAD"], checkout), expected_head)
                    self.assertEqual(run(["git", "status", "--porcelain", "--untracked-files=no"], checkout), "")
                    example = checkout / "src/engine/examples" / f"{EXAMPLE}.rs"
                    example.parent.mkdir(exist_ok=True)
                    self.assertFalse(example.exists())
                    wrapper = source
                    if label == "current":
                        wrapper = wrapper.replace("// CURRENT_RECOVERY_IMPLEMENTATION", (FIXTURES / "recover.rs").read_text())
                        wrapper = wrapper.replace("// CURRENT_RECOVERY_COMMAND", '"recover" => { recover(root); 0 },')
                    example.write_text(wrapper)
                    target = work / f"target-{label}"
                    run(["cargo", "build", "--locked", "--offline", "-p", "storage-engine", "--example", EXAMPLE,
                         "--target-dir", str(target)], checkout)
                    binaries[label] = target / "debug/examples" / EXAMPLE
                    records.append({"binary": label, "source_sha": expected_head,
                                    "binary_sha256": hashlib.sha256(binaries[label].read_bytes()).hexdigest()})
                for producer, consumer in [("current", "old"), ("old", "current")]:
                    material = work / f"material-{producer}"
                    material.mkdir(mode=0o700)
                    run([str(binaries[producer]), "seed", str(material)], ROOT, maximum=90)
                    run([str(binaries[producer]), "open", str(material)], ROOT, maximum=90)
                    for operation in ["open", "admit"]:
                        before = image(material)
                        run([str(binaries[consumer]), operation, str(material)], ROOT, expected=73, maximum=90)
                        after = image(material)
                        (evidence / f"{producer}-{consumer}-{operation}-before.json").write_text(json.dumps(before))
                        (evidence / f"{producer}-{consumer}-{operation}-after.json").write_text(json.dumps(after))
                        self.assertEqual(after, before, f"{consumer} {operation} changed retained material")
                    run([str(binaries[producer]), "open", str(material)], ROOT, maximum=90)
                    if producer == "current":
                        run([str(binaries[producer]), "recover", str(material)], ROOT, maximum=90)
                        before = image(material)
                        run([str(binaries[consumer]), "open", str(material)], ROOT, expected=73, maximum=90)
                        after = image(material)
                        (evidence / "recovery-before.json").write_text(json.dumps(before))
                        (evidence / "recovery-after.json").write_text(json.dumps(after))
                        self.assertEqual(after, before, "old Engine changed the new recovery carrier")
                        run([str(binaries[producer]), "open", str(material)], ROOT, maximum=90)
                (evidence / "operations.json").write_text(json.dumps(records, indent=2))
            finally:
                for checkout in [current, old]:
                    if checkout.exists():
                        subprocess.run(["git", "worktree", "remove", "--force", str(checkout)], cwd=ROOT,
                                       check=True, timeout=30)
        print(f"actual cross-revision evidence: {evidence}", flush=True)
