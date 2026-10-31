"""Static wiring of the foundation behaviour gate: suites, inventory, features and owners."""
import importlib.util
import hashlib
import json
from pathlib import Path
import re
import tempfile
import tomllib
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("behaviour_gate", ROOT / ".github/scripts/check-foundation-behaviour.py")
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)

# Owners whose files need proof-maintenance receipts or a new adapter (docs/EXTERNAL_TEST_SOURCES.md).
PROOF_BOUND = ("tokio", "synchronization", "system-entropy", "libc", "memchr", "mio", "rustc-demangle",
               "ring", "http-transport")


class FoundationBehaviourGateTests(unittest.TestCase):
    def test_gate_runs_once_in_the_async_shard(self):
        script = (ROOT / ".github/scripts/check-foundation.sh").read_text()
        async_case = script.split("  foundation-async)\n", 1)[1].split(";;", 1)[0]
        self.assertEqual(script.count("check-foundation-behaviour.py"), 1)
        self.assertIn("python3 .github/scripts/check-foundation-behaviour.py", async_case)

    def test_suite_sources_exist_and_unexecuted_material_is_explicit(self):
        executed = set()
        for suite in (*GATE.INTEGRATION_SUITES, *GATE.STANDALONE_SUITES):
            for path in suite.tests:
                self.assertTrue((ROOT / path).is_file(), path)
                executed.add(path)
            for path in suite.examples:
                self.assertTrue((GATE.foundation_path(ROOT, suite.subject) / path).is_file(), path)
        for path, reason in GATE.NOT_EXECUTED.items():
            self.assertTrue((ROOT / path).is_file(), path)
            self.assertNotIn(path, executed)
            self.assertTrue(reason.strip())
        self.assertEqual(len(GATE.suite_keys()), len(set(GATE.suite_keys())))

    def test_inventory_is_generated_for_exactly_the_suite_table(self):
        inventory = json.loads(GATE.INVENTORY.read_text())
        self.assertEqual(inventory["schema"], 1)
        self.assertEqual(sorted(inventory["suites"]), sorted(GATE.suite_keys()))
        for suite in GATE.INTEGRATION_SUITES:
            recorded = inventory["suites"]["integration:" + suite.subject]
            self.assertEqual(sorted(recorded["targets"]), sorted(map(GATE.target_name, suite.tests)))
        for suite in GATE.STANDALONE_SUITES:
            recorded = inventory["suites"]["standalone:" + suite.subject]
            expected = list(map(GATE.target_name, suite.tests)) if suite.tests else ["lib"]
            self.assertEqual(sorted(recorded["targets"]), sorted(expected))
        for key, value in inventory["suites"].items():
            for target, names in value["targets"].items():
                self.assertTrue(names, f"{key}/{target} would accept an empty binary")
                self.assertEqual(names, sorted(set(names)), f"{key}/{target}")
        self.assertEqual(GATE.INVENTORY.read_text(), GATE.inventory_text(inventory["suites"]))

    def test_integration_members_use_production_features_not_all_features(self):
        production = GATE.production_features()
        flume = next(suite for suite in GATE.INTEGRATION_SUITES if suite.subject == "flume")
        manifest = tomllib.loads(GATE.member_manifest(flume, production))
        self.assertEqual(production["flume"], ["async"])
        self.assertEqual(manifest["dependencies"]["flume"]["features"], ["async"])
        self.assertFalse(manifest["dependencies"]["flume"]["default-features"])
        self.assertEqual(manifest["features"]["default"], ["async"])
        declared = tomllib.loads((GATE.foundation_path(ROOT, "flume") / "Cargo.toml").read_text())["features"]
        self.assertEqual(set(manifest["features"]) - {"default"}, set(declared) - {"default"})
        source = (ROOT / ".github/scripts/check-foundation-behaviour.py").read_text()
        self.assertNotIn("--all-features", source)

    def test_suites_do_not_execute_material_of_receipt_bound_owners(self):
        for suite in (*GATE.INTEGRATION_SUITES, *GATE.STANDALONE_SUITES):
            self.assertNotIn(suite.subject, PROOF_BOUND)
            for path in suite.tests:
                for owner in PROOF_BOUND:
                    self.assertFalse(path.startswith(f"test/foundation/{owner}/"), path)
        self.assertFalse(set(GATE.UNIT_SUITES) & set(PROOF_BOUND))

    def test_standalone_members_preserve_real_code_build_cfg_and_external_test_bytes(self):
        suite = next(suite for suite in GATE.STANDALONE_SUITES if suite.subject == "proc-macro2")
        subject = GATE.foundation_path(ROOT, suite.subject)
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            manifest = tomllib.loads(GATE.standalone_member(suite, work))
            copied = work / subject.relative_to(ROOT)
            for relative in ("src/lib.rs", "build.rs", "src/probe/proc_macro_span.rs"):
                self.assertEqual((copied / relative).read_bytes(), (subject / relative).read_bytes())
            self.assertEqual(Path(manifest["dependencies"]["unicode-ident"]["path"]),
                             GATE.foundation_path(ROOT, "unicode-ident"))
            self.assertEqual(manifest["features"], tomllib.loads((subject / "Cargo.toml").read_text())["features"])
            self.assertEqual(len(manifest["test"]), len(suite.tests))
            for target, relative in zip(manifest["test"], suite.tests):
                self.assertEqual((copied / target["path"]).resolve(), work / relative)
                self.assertEqual((work / relative).read_bytes(), (ROOT / relative).read_bytes())
            self.assertFalse((subject / "Cargo.lock").exists())

    def test_legacy_edition_test_keeps_its_visibility_semantics(self):
        suite = next(suite for suite in GATE.INTEGRATION_SUITES if suite.subject == "lazy_static")
        manifest = tomllib.loads(GATE.member_manifest(suite, GATE.production_features()))
        self.assertEqual(manifest["package"]["edition"], "2015")

    def test_documentation_reports_ignored_and_compile_only_examples_honestly(self):
        log = Mock()
        log.run.side_effect = [
            'example (line 1): test\nexample (line 2): test\n',
            'test example (line 1) - compile ... ok\n'
            'test example (line 2) ... ignored\n'
            'test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n',
        ]
        self.assertEqual(GATE.cargo_doctests(log, ROOT, ['-p', 'example'], 'doc'),
                         {'executed': [], 'compile_only': ['example (line 1)'], 'ignored': ['example (line 2)']})
        log.run.side_effect = [
            'example (line 1): test\n',
            'test example (line 1) ... ignored\n'
            'test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n',
        ]
        with self.assertRaisesRegex(AssertionError, 'no documentation example checked'):
            GATE.cargo_doctests(log, ROOT, ['-p', 'example'], 'doc')

    def test_append_executes_only_new_keys_and_never_overwrites_prior_evidence(self):
        old = {'unit:old': {'subject': 'old', 'features': [], 'targets': {'lib': ['old_test']}}}
        new = {'unit:new': {'subject': 'new', 'features': [], 'targets': {'lib': ['new_test']}}}
        with tempfile.TemporaryDirectory() as temporary:
            inventory = Path(temporary) / 'inventory.json'
            inventory.write_text(GATE.inventory_text(old))
            with patch.object(GATE, 'INVENTORY', inventory), \
                    patch.object(GATE, 'suite_keys', return_value=['unit:old', 'unit:new']), \
                    patch.object(GATE, 'run_suites', return_value=new) as run, \
                    patch('sys.argv', ['gate', '--append']), patch('builtins.print'):
                GATE.main()
            run.assert_called_once_with({'unit:new'})
            self.assertEqual(json.loads(inventory.read_text())['suites'], old | new)
            with patch.object(GATE, 'INVENTORY', inventory), \
                    patch.object(GATE, 'suite_keys', return_value=['unit:new']), \
                    patch.object(GATE, 'run_suites') as run, \
                    patch('sys.argv', ['gate', '--append']):
                with self.assertRaisesRegex(AssertionError, 'only additional suite keys'):
                    GATE.main()
            run.assert_not_called()

    def test_ansi_readme_adapts_only_the_retired_variant_spelling_and_keeps_its_origin(self):
        original = ROOT / 'docs/upstream/nu-ansi-term-0.50.3/README.md'
        current = ROOT / 'test/foundation/nu-ansi-term/modules/readme.rs'
        adapted = original.read_text().replace('Color::RGB', 'Color::Rgb').replace('RGB(', 'Rgb(')
        adapted = adapted.replace('nu-ansi-term = "0.49"\n\n# optional gnu-legacy mode',
                                  'nu-ansi-term = "0.49"\n```\n\n```toml\n[dependencies]\n'
                                  '# optional gnu-legacy mode')
        documentation = (ROOT / 'docs/validation/nu-ansi-term-examples.md').read_text()
        self.assertTrue(documentation.endswith(adapted))
        self.assertIn('原始 README', documentation[:-len(adapted)])
        self.assertEqual(current.read_text(),
                         '#![doc = include_str!("../../../../docs/validation/nu-ansi-term-examples.md")]\n')
        mapping = json.loads((ROOT / 'compliance/external-tests/foundation.json').read_text())
        owner = next(o for o in mapping['owners'] if o['directory'].endswith('/nu-ansi-term'))
        row = next(r for r in owner['files'] if r['path'] == str(current.relative_to(ROOT)))
        self.assertEqual(row['source_kind'], 'project-authored')
        self.assertEqual(row['source'], row['path'])
        self.assertEqual(row['source_sha256_before'], hashlib.sha256(current.read_bytes()).hexdigest())

    def test_every_retained_ansi_readme_rust_block_is_present_in_the_doctest_inventory(self):
        suite = next(s for s in GATE.STANDALONE_SUITES if s.subject == 'nu-ansi-term')
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            GATE.standalone_member(suite, work)
            subject = GATE.foundation_path(ROOT, suite.subject).relative_to(ROOT)
            self.assertTrue((work / subject / 'Cargo.toml').is_file())
            documentation = 'docs/validation/nu-ansi-term-examples.md'
            self.assertEqual((work / documentation).read_bytes(), (ROOT / documentation).read_bytes())
        original = (ROOT / 'docs/upstream/nu-ansi-term-0.50.3/README.md').read_text()
        attributes = re.findall(r'^```rust([^\n]*)', original, re.M)
        ignored = sum('ignore' in tags.split(',') for tags in attributes)
        documentation = json.loads(GATE.INVENTORY.read_text())['suites']['standalone:nu-ansi-term']['doctests']
        selected = {key: [name for name in names if ' - readme_doctests (' in name]
                    for key, names in documentation.items()}
        self.assertEqual(len(selected['ignored']), ignored)
        self.assertEqual(len(selected['executed']), len(attributes) - ignored)
        self.assertFalse(selected['compile_only'])

    def test_comparison_and_date_suites_use_only_retained_functional_targets(self):
        self.assertIn("cmov", GATE.UNIT_SUITES)
        self.assertIn("httpdate", GATE.UNIT_SUITES)
        # On x86_64 CI, lib tests are instruction-count gated or AArch64-only, not a functional suite.
        self.assertNotIn("constant_time_eq", GATE.UNIT_SUITES)
        suites = {suite.subject: suite for suite in GATE.INTEGRATION_SUITES}
        expected = {
            "cmov": ("test/foundation/cmov/tests/core_impls.rs",
                     "test/foundation/cmov/tests/regression.rs"),
            "constant_time_eq": ("test/foundation/constant_time_eq/tests/exhaustive.rs",),
        }
        for name, paths in expected.items():
            with self.subTest(subject=name):
                self.assertEqual(suites[name].tests, paths)
                self.assertEqual(suites[name].providers, ())
                self.assertEqual(suites[name].examples, ())

    def test_comparison_and_date_features_match_real_consumers(self):
        production = GATE.production_features()
        inventory = json.loads(GATE.INVENTORY.read_text())["suites"]
        for name in ("cmov", "httpdate"):
            with self.subTest(subject=name):
                self.assertEqual(production[name], [])
                self.assertEqual(inventory["unit:" + name]["features"], [])
        suites = {suite.subject: suite for suite in GATE.INTEGRATION_SUITES}
        for name, features in (("cmov", []), ("constant_time_eq", ["std"])):
            with self.subTest(subject=name):
                self.assertEqual(production[name], features)
                manifest = tomllib.loads(GATE.member_manifest(suites[name], production))
                self.assertEqual(set(manifest["dependencies"]), {name})
                self.assertEqual(manifest["dependencies"][name]["features"], features)
                self.assertFalse(manifest["dependencies"][name]["default-features"])
                self.assertEqual(manifest["features"]["default"], features)
                self.assertEqual(inventory["integration:" + name]["features"], features)

    def test_missing_comparison_providers_remain_explicitly_unexecuted(self):
        expected = {"test/foundation/cmov/tests/proptests.rs": "proptest"}
        expected.update({f"test/foundation/constant_time_eq/tests/{target}.rs": "count_instructions"
                         for target in ("count_instructions", "count_instructions_classic",
                                        "count_instructions_generic")})
        for path, provider in expected.items():
            with self.subTest(path=path):
                self.assertIn(provider, GATE.NOT_EXECUTED[path])

    def test_retained_tests_use_local_providers_only(self):
        absent = re.compile(r"\b(?:rand|easy_parallel|futures_test|pin_utils|async_std|crossbeam_channel)::")
        for suite in GATE.INTEGRATION_SUITES:
            for path in suite.tests:
                self.assertIsNone(absent.search((ROOT / path).read_text()), path)


if __name__ == "__main__":
    unittest.main()
