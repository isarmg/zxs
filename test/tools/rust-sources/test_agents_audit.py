"""The syntax inventory deliberately does not classify candidates as violations."""
import importlib.util
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("agents_audit", ROOT / "tools/rust-sources/agents_audit.py")
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class CandidatePatterns(unittest.TestCase):
    def test_ignored_result_excludes_match_arm(self):
        pattern = audit.PATTERNS["ignored_result_syntax"]
        for line in ["let _ = write();", "_ = rx.await;", "value.ok();", "if let Ok(value) = call() {"]:
            self.assertIsNotNone(re.search(pattern, line), line)
        for line in ["_ => {}", "_ == value", "value_name = 3"]:
            self.assertIsNone(re.search(pattern, line), line)

    def test_default_and_error_candidates_are_detected(self):
        self.assertIsNotNone(re.search(audit.PATTERNS["default_fallback_syntax"], 'value.unwrap_or("")'))
        self.assertIsNotNone(re.search(audit.PATTERNS["erased_error_candidate"], 'call.map_err(|error| error.to_string())'))


if __name__ == "__main__":
    unittest.main()
