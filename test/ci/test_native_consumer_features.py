"""Consumer capability checks must inspect real, non-unified production/dev graphs."""
from pathlib import Path
import runpy
import subprocess
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
GATE = runpy.run_path(str(ROOT / ".github/scripts/check-native-consumer-features.py"))
MINIMAL = "rustix v1.1.5 (/source/rustix)|alloc,default,fs,process,std\n"
HTTP = "http-routing v0.8.9 (/source/http-routing)|http1,json,query,tokio,tower-log,tracing\n"
HELPER_HTTP = "http-routing v0.8.9 (/source/http-routing)|http1,json,tokio\n"


ENGINE = "storage-engine v0.1.1 (/source/engine)|\n"
FAULT_ENGINE = ENGINE.replace("|\n", "|fault-injection\n")


class NativeConsumerFeaturesTests(unittest.TestCase):
    def test_required_filesystem_and_query_capabilities_remain(self):
        for consumer in GATE["CONSUMERS"]:
            for edges in GATE["EDGES"]:
                engine = FAULT_ENGINE if edges == "normal,build,dev" else ENGINE
                http = HELPER_HTTP if consumer == "storage-helper" and edges == "normal,build" else HTTP
                GATE["validate"](MINIMAL + http + engine, consumer, edges)
        GATE["validate"](MINIMAL, "storage-client", "normal,build")

    def test_mount_and_form_regressions_are_rejected_in_every_occurrence(self):
        for extra in (MINIMAL.replace("fs,", "mount,fs,"), HTTP.replace("json,", "form,json,")):
            with self.subTest(extra=extra):
                with self.assertRaisesRegex(AssertionError, "unexpected .* capabilities"):
                    GATE["validate"](MINIMAL + HTTP + FAULT_ENGINE + extra, "storage-server", "normal,build,dev")

    def test_missing_graph_entries_or_required_features_cannot_pass(self):
        for output in ("", HTTP, MINIMAL, MINIMAL + HTTP.replace("query,", ""),
                       MINIMAL.replace("process,", "") + HTTP):
            with self.subTest(output=output):
                with self.assertRaises(AssertionError):
                    GATE["validate"](output, "storage-server", "normal,build")
        with self.assertRaisesRegex(AssertionError, "missing expected graph entries"):
            GATE["validate"](MINIMAL, "storage-helper", "normal,build,dev")

    def test_helper_production_json_console_is_distinct_from_server_dev_graph(self):
        GATE["validate"](MINIMAL + HELPER_HTTP + ENGINE, "storage-helper", "normal,build")
        for http in ("", HELPER_HTTP.replace("json,", ""),
                     HELPER_HTTP.replace("http1,", ""), HELPER_HTTP.replace("tokio", ""),
                     HELPER_HTTP.replace("json,", "json,form,"), HTTP):
            with self.subTest(http=http), self.assertRaises(AssertionError):
                GATE["validate"](MINIMAL + http + ENGINE, "storage-helper", "normal,build")
        with self.assertRaisesRegex(AssertionError, "unexpected http-routing capabilities"):
            GATE["validate"](MINIMAL + HELPER_HTTP + ENGINE, "storage-helper", "normal,build,dev")

    def test_each_consumer_and_dependency_scope_is_selected_separately(self):
        def result(command, **kwargs):
            engine = FAULT_ENGINE if "normal,build,dev" in command else ENGINE
            http = HELPER_HTTP if "storage-helper" in command and "normal,build" in command else HTTP
            return subprocess.CompletedProcess(command, 0, MINIMAL + http + engine)
        with mock.patch.object(GATE["subprocess"], "run", side_effect=result) as run:
            GATE["check"](ROOT)
        expected = [mock.call(
            ["cargo", "tree", "--locked", "--offline", "-p", consumer,
             "--target", GATE["TARGET"], "--edges", edges, "--prefix", "none", "--format", "{p}|{f}"],
            cwd=ROOT, text=True, capture_output=True, check=True, timeout=120,
        ) for consumer in GATE["CONSUMERS"] for edges in GATE["EDGES"]]
        self.assertEqual(run.call_args_list, expected)

    def test_fault_injection_is_test_only_and_cannot_silently_disappear(self):
        for consumer in GATE["CONSUMERS"]:
            with self.subTest(consumer=consumer):
                with self.assertRaisesRegex(AssertionError, "production graph enables"):
                    http = HELPER_HTTP if consumer == "storage-helper" else HTTP
                    GATE["validate"](MINIMAL + http + FAULT_ENGINE, consumer, "normal,build")
        for consumer in ("storage-engine", "storage-server"):
            with self.assertRaisesRegex(AssertionError, "test graph lacks"):
                GATE["validate"](MINIMAL + HTTP + ENGINE, consumer, "normal,build,dev")

    def test_cargo_failure_is_not_hidden(self):
        with mock.patch.object(GATE["subprocess"], "run",
                               side_effect=subprocess.CalledProcessError(101, ["cargo"])):
            with self.assertRaises(subprocess.CalledProcessError):
                GATE["check"](ROOT)


if __name__ == "__main__":
    unittest.main()
