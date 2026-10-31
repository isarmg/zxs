"""The browser executor is real js-sys code, not a renamed compatibility facade."""
import base64
import gzip
import hashlib
import json
import importlib.util
import os
import subprocess
import tempfile
from unittest import mock
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class BrowserFuturesRetirement(unittest.TestCase):
    def test_exact_retirement_material_is_preserved(self):
        plan = json.loads((ROOT / 'compliance/browser-futures-retirement-plan.json').read_text())
        self.assertEqual(plan['schema'], 1)
        self.assertEqual(len(plan['packages']), 1)
        item = plan['packages'][0]
        owner = 'src/foundation/wasm-bindgen-futures'
        self.assertEqual(item['directory'], owner)
        self.assertFalse((ROOT / owner).exists())
        archive = json.loads(gzip.decompress(
            (ROOT / 'compliance/wasm-bindgen-futures-0.4.79-retirement.json.gz').read_bytes()))
        self.assertEqual(archive['schema_version'], 1)
        self.assertEqual(archive['package'], item['package'])
        self.assertIsNone(archive['external_tests'])
        self.assertEqual(archive['retirement'], {
            key: item[key] for key in ('bytes', 'category', 'file_count', 'reason')})
        expected = {owner + '/' + path: digest
                    for path, digest in item['package']['files_sha256'].items()}
        self.assertEqual(set(archive['files']), set(expected))
        total = 0
        for path, encoded in archive['files'].items():
            data = base64.b64decode(encoded, validate=True)
            self.assertEqual(hashlib.sha256(data).hexdigest(), expected[path])
            self.assertEqual(base64.b64encode(data).decode(), encoded)
            total += len(data)
        self.assertEqual(total, item['bytes'])
        self.assertEqual(len(expected), item['file_count'])
        for path in item['package']['license_materials']:
            self.assertTrue((ROOT / path).is_file())

    def test_active_manifests_do_not_depend_on_shim(self):
        for path in (ROOT / 'src').rglob('Cargo.toml'):
            self.assertNotIn('wasm-bindgen-futures', path.read_text(), str(path))
        script = (ROOT / '.github/scripts/check-csr-scope.py').read_text()
        self.assertNotIn('wasm-bindgen-futures', script)
        macros = ROOT / 'src/foundation/web-runtime/wasm-binding-macros/src'
        for name in ('ast.rs', 'codegen.rs', 'encode.rs', 'lib.rs'):
            self.assertNotIn('wasm_bindgen_futures', (macros / name).read_text())
        for path in macros.glob('*.rs'):
            self.assertNotIn('WASM_BINDGEN_USE_JS_SYS', path.read_text())
            self.assertNotIn('use_js_sys_futures', path.read_text())

    def test_ci_and_release_require_browser_execution_without_disabling_security(self):
        for filename, job, next_job in (
            ('ci.yml', 'tests', 'web-production'),
            ('release.yml', 'verify', 'linux-web'),
        ):
            with self.subTest(workflow=filename):
                workflow = (ROOT / '.github/workflows' / filename).read_text()
                job_source = workflow.split(f'  {job}:\n', 1)[1].split(f'  {next_job}:\n', 1)[0]
                self.assertIn("    env:\n      ZXS_REQUIRE_BROWSER: '1'", job_source)
                self.assertIn("if: matrix.suite == 'foundation-language'\n"
                              "        run: bash .github/scripts/prepare-browser-contract.sh", job_source)
                if filename == 'ci.yml':
                    self.assertIn('suite: ${{ fromJSON(needs.static.outputs.suites) }}',
                                  job_source.split('    steps:', 1)[0])
                    spec = importlib.util.spec_from_file_location(
                        'browser_ci_selector', ROOT / '.github/scripts/select-ci-suites.py')
                    selector = importlib.util.module_from_spec(spec)
                    spec.loader.exec_module(selector)
                    self.assertIn('foundation-language', selector.ALL)
                    self.assertIn('security', selector.ALL)
                    self.assertNotIn('security', selector.FOUNDATION)
                    for path in ('src/engine/src/lib.rs', 'src/server/src/main.rs',
                                 'tools/history-scale-bench/src/main.rs'):
                        self.assertIn('foundation-language', selector.affected(path), path)
                    # Actual browser/runtime input changes must force the full matrix,
                    # not merely preserve a similarly named unrelated suite.
                    for path in ('src/foundation/web-runtime/js-sys/src/lib.rs',
                                 'test/wasm-binding-macros/browser-runtime.html',
                                 '.github/scripts/prepare-browser-contract.sh'):
                        self.assertIsNone(selector.affected(path), path)
                    fallback = selector.select(ROOT, 'workflow_dispatch', {}, '', '')
                    self.assertIn('foundation-language', fallback['suites'])
                    self.assertIn('security', fallback['suites'])
                else:
                    self.assertIn('foundation-language', job_source.split('    steps:', 1)[0])
                self.assertLess(job_source.index('prepare-browser-contract.sh'),
                                job_source.index('bash .github/scripts/check.sh'))
        setup = (ROOT / '.github/scripts/prepare-browser-contract.sh').read_text()
        self.assertIn('command -v google-chrome', setup)
        self.assertIn('echo "ZXS_CHROMIUM=$chrome"', setup)
        script = (ROOT / '.github/scripts/check-web-bindgen-scope.py').read_text()
        for flag in ('--no-sandbox', '--disable-setuid-sandbox', '--disable-web-security'):
            self.assertNotIn(flag, script + setup)

    def test_shared_browser_setup_exports_only_a_working_browser(self):
        script = ROOT / '.github/scripts/prepare-browser-contract.sh'
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            browser = directory / 'google-chrome'
            environment = directory / 'environment'
            env = {'PATH': str(directory), 'GITHUB_ENV': str(environment)}
            result = subprocess.run(['/bin/bash', str(script)], env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(environment.exists())
            browser.write_text('#!/bin/sh\nexit 1\n')
            browser.chmod(0o700)
            result = subprocess.run(['/bin/bash', str(script)], env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(environment.exists())
            browser.write_text('#!/bin/sh\necho Chrome test fixture\n')
            result = subprocess.run(['/bin/bash', str(script)], env=env, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(environment.read_text(), f'ZXS_CHROMIUM={browser}\n')

    def load_browser_check(self):
        spec = importlib.util.spec_from_file_location(
            'web_bindgen_scope', ROOT / '.github/scripts/check-web-bindgen-scope.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_required_browser_missing_fails_before_compilation(self):
        module = self.load_browser_check()
        with mock.patch.dict(os.environ, {'ZXS_REQUIRE_BROWSER': '1'}, clear=True), \
                mock.patch.object(module, 'cargo') as cargo:
            with self.assertRaisesRegex(RuntimeError, 'ZXS_CHROMIUM is required'):
                module.check()
            cargo.assert_not_called()

    def test_optional_local_browser_probe_still_skips(self):
        module = self.load_browser_check()
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertEqual(module.browser_check(Path('/unused')), 'browser execution not requested')

    def test_configured_missing_browser_fails_even_in_local_mode(self):
        module = self.load_browser_check()
        for required in ('0', '1'):
            with self.subTest(required=required), mock.patch.dict(os.environ, {
                'ZXS_REQUIRE_BROWSER': required, 'ZXS_CHROMIUM': '/nonexistent/browser',
            }, clear=True):
                with self.assertRaisesRegex(RuntimeError, 'not executable'):
                    module.browser_configuration()

    def test_required_browser_executes_fixture_and_rejects_failed_runtime(self):
        module = self.load_browser_check()
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.dict(os.environ, {'ZXS_REQUIRE_BROWSER': '1', 'ZXS_CHROMIUM': '/browser'}, clear=True), \
                mock.patch.object(module.shutil, 'which', return_value='/browser'), \
                mock.patch.object(module.subprocess, 'run') as run:
            output = Path(directory)
            checks = [
                'generated async export', 'generated async import', 'JsFuture and Promise IntoFuture',
                'future_to_promise resolution and rejection', 'spawn_local next microtask',
                'empty Unicode and large strings', 'async class method and retained state',
                'task dispatch Send and local futures', 'task dispatch initialization and tick',
            ]
            def successful(checks):
                return subprocess.CompletedProcess([], 0, '<body>' + json.dumps(
                    {'status': 'passed', 'checks': checks}, separators=(',', ':')) + '</body>', '')
            run.return_value = successful(checks)
            self.assertEqual(module.browser_check(output), '9 actual Chromium async runtime checks passed')
            self.assertEqual((output / 'index.html').read_text(),
                             (ROOT / 'test/wasm-binding-macros/browser-runtime.html').read_text())
            self.assertEqual(run.call_args.args[0][0], '/browser')
            self.assertIn('--dump-dom', run.call_args.args[0])
            run.return_value = successful(checks[:7])
            with self.assertRaises(AssertionError):
                module.browser_check(output)
            run.return_value = successful(checks + [checks[-1]])
            with self.assertRaises(AssertionError):
                module.browser_check(output)
            run.return_value = subprocess.CompletedProcess([], 0, '<body>{"status":"failed"}</body>', '')
            with self.assertRaises(AssertionError):
                module.browser_check(output)

    def test_dispatch_preserves_std_without_enabling_eval(self):
        manifest = tomllib.loads((ROOT / 'src/foundation/concurrency/task-dispatch/Cargo.toml').read_text())
        dependency = manifest['dependencies']['js-sys']
        self.assertEqual(dependency['path'], '../../web-runtime/js-sys')
        self.assertTrue(dependency['optional'])
        self.assertFalse(dependency['default-features'])
        self.assertEqual(dependency['features'], ['std'])
        self.assertEqual(manifest['features']['wasm-bindgen'], ['dep:js-sys'])
        js = tomllib.loads((ROOT / 'src/foundation/web-runtime/js-sys/Cargo.toml').read_text())
        self.assertIn('wasm-bindgen/std', js['features']['std'])


if __name__ == '__main__':
    unittest.main()
