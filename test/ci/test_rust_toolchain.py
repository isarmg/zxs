"""Keep the compiler pin, first-party minimum and targeted compiler scope coherent."""
import json
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class RustToolchain(unittest.TestCase):
    def test_stable_pin_matches_workspace_minimum(self):
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
        self.assertEqual(toolchain["channel"], "1.100.0")
        self.assertEqual(workspace["package"]["rust-version"], "1.100")
        self.assertEqual(toolchain["profile"], "minimal")
        self.assertCountEqual(toolchain["components"], ["rustfmt", "clippy"])
        self.assertEqual(workspace["package"]["edition"], "2024")

    def test_upgrade_selects_native_and_wasm_consumers(self):
        policy = json.loads((ROOT / ".github/ci/targeted-tests.json").read_text())
        for path in ["rust-toolchain.toml", "Cargo.toml"]:
            self.assertIn({"path": path, "groups": ["rust-toolchain-upgrade"]}, policy["rules"])
        targets = policy["groups"]["rust-toolchain-upgrade"]["rust_targets"]
        self.assertTrue(any(t["package"] == "storage-server" and t.get("name") == "zxs" for t in targets))
        self.assertTrue(any(t["package"] == "storage-helper" for t in targets))
        self.assertTrue(any(t.get("compile_target") == "wasm32-unknown-unknown" for t in targets))
        self.assertTrue(any(t["package"] == "smb-protocol" and t.get("whole") for t in targets))
