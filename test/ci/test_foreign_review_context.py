"""Source wiring and host collector models, not real browser accessibility."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ForeignReviewContext(unittest.TestCase):
    def test_snapshot_collector_preserves_multi_origin_recovery_context(self):
        subprocess.run([
            "node", str(ROOT / "test/tools/browser-smoke/unit/foreign_review_context_test.mjs")
        ], cwd=ROOT, check=True)

    def test_original_operation_and_no_reservation_scope_describe_recovery_controls(self):
        view = (ROOT / "src/web/src/admin/storage/foreign/plan/view.rs").read_text()
        self.assertEqual(view.count('id="zxs-foreign-plan-operation"'), 1)
        self.assertEqual(view.count('id="zxs-foreign-plan-scope"'), 1)
        description = 'custom_attribute("aria-describedby","zxs-foreign-plan-operation zxs-foreign-plan-scope")'
        for control in ("confirm", "save", "retry", "status", "revalidate"):
            marker = f'data-foreign-plan-{control}=""'
            element = view[view.index(marker):view.index(">", view.index(marker))]
            self.assertIn(description, element, control)

    def test_request_completion_uses_the_model_generation_and_session_gates(self):
        actions = (ROOT / "src/web/src/admin/storage/foreign/plan/actions.rs").read_text()
        load = actions[actions.index("pub(super) fn load("):]
        self.assertIn("model.read_request(action)?", load)
        self.assertIn("state.session_epoch != epoch", load)
        self.assertIn("accept_read_response(state, epoch, &result)", load)
        self.assertIn(".finish_read(generation, result)", load)
        for forbidden in ("/execute", "/executions/", "/execution-intents/"):
            self.assertNotIn(forbidden, actions)


if __name__ == "__main__":
    unittest.main()
