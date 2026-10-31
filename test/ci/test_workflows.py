"""Keep runner-only context and environment setup out of early workflow env."""
import importlib.util
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

SCRIPTS = (Path(__file__).resolve().parents[2] / ".github/scripts")
SPEC = importlib.util.spec_from_file_location("workflow_checker", SCRIPTS / "check-workflows.py")
checker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(checker)


class WorkflowContexts(unittest.TestCase):
    def test_job_display_name_colon_requires_yaml_quotes(self):
        bad = "jobs:\n  browser-smoke:\n    name: Standalone ELF: Debian 13 server + host Chrome\n"
        self.assertTrue(any("must be quoted" in error for error in checker.check(bad)))
        good = bad.replace("name: Standalone ELF: Debian 13 server + host Chrome", 'name: "Standalone ELF: Debian 13 server + host Chrome"')
        self.assertEqual(checker.check(good), [])
        self.assertEqual(checker.check("jobs:\n  job:\n    steps:\n      - run: |\n          name: shell: value\n"), [])

    def test_repository_workflows_use_available_early_contexts(self):
        for path in (SCRIPTS.parent / "workflows").glob("*.y*ml"):
            with self.subTest(path=path.name):
                self.assertEqual(checker.check(path.read_text()), [])

    def test_every_rust_job_initializes_cargo_home_before_toolchain_use(self):
        expected = {
            "ci.yml": {"tests", "web-production", "browser-smoke"},
            "release.yml": {"version", "verify", "linux-web"},
        }
        for name, rust_jobs in expected.items():
            source = (SCRIPTS.parent / "workflows" / name).read_text()
            parts = re.split(r"^  ([A-Za-z0-9_-]+):\s*$", source.split("\njobs:\n", 1)[1], flags=re.MULTILINE)
            jobs = dict(zip(parts[1::2], parts[2::2]))
            self.assertIn("bash .github/scripts/check.sh static", jobs["static"])
            self.assertNotRegex(jobs["static"], r"\b(?:cargo|rustup|rustc)\s")
            for job in rust_jobs:
                with self.subTest(workflow=name, job=job):
                    body = jobs[job]
                    self.assertLess(body.index("initialize-cargo-home.sh"), body.index("rustup "))

    def test_release_and_ci_share_same_commit_samba_sdk_gate(self):
        workflows = SCRIPTS.parent / "workflows"
        for name in ("ci.yml", "release.yml"):
            source = (workflows / name).read_text()
            self.assertIn("uses: ./.github/workflows/smb-sdk.yml", source)
        release = (workflows / "release.yml").read_text()
        self.assertIn("needs: [version, verify, linux-web, smb-sdk]", release)
        sdk = (workflows / "smb-sdk.yml").read_text()
        self.assertIn("workflow_call:", sdk)
        self.assertIn("bash .github/scripts/smb-sdk.sh", sdk)
        self.assertLess(sdk.index("initialize-cargo-home.sh"), sdk.index("rustup "))
        self.assertNotIn("ref:", sdk)  # checkout must use the caller's commit

    def test_security_cache_is_narrow_and_never_skips_the_suite(self):
        expected = """name: Cache verified security scanner
        if: matrix.suite == 'security'
        uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: |
            target/security-tools/bin/cargo-audit
            target/security-tools/verified-build.json
          key: security-audit-ubuntu24.04-${{ runner.os }}-${{ runner.arch }}-${{ hashFiles('rust-toolchain.toml', '.github/scripts/install-security-audit.py', '.github/security-tools/cargo-audit.json', '.github/security-tools/cargo-audit-0.22.2.lock') }}"""
        for filename, job, suite_name in (
            ("ci.yml", "tests", "Run locked test suite"),
            ("release.yml", "verify", "Run portable test suite and contract checks"),
        ):
            with self.subTest(workflow=filename):
                source = (SCRIPTS.parent / "workflows" / filename).read_text()
                parts = re.split(r"^  ([A-Za-z0-9_-]+):\s*$",
                                 source.split("\njobs:\n", 1)[1], flags=re.MULTILINE)
                body = dict(zip(parts[1::2], parts[2::2]))[job]
                self.assertIn("    runs-on: ubuntu-24.04\n", body)
                steps = [step.strip() for step in re.split(r"^      - ", body, flags=re.MULTILINE)[1:]]
                cache = [step for step in steps if "security-audit-" in step]
                self.assertEqual(cache, [expected])  # Exact inputs/paths; no restore-keys or cache-hit bypass.
                following = steps[steps.index(cache[0]) + 1]
                self.assertEqual(following, f"name: {suite_name}\n"
                                 '        run: bash .github/scripts/check.sh "${{ matrix.suite }}"')

    def test_workflow_env_rejects_runner_and_other_job_only_contexts(self):
        for expression in ("runner.temp", "Runner['temp']", "toJSON(runner)",
                           "matrix.target", "needs.build.outputs.path", "env.OTHER"):
            with self.subTest(expression=expression):
                errors = checker.check(f"env:\n  CARGO_HOME: ${{{{ {expression} }}}}/cargo\njobs:\n  test:\n")
                self.assertEqual(len(errors), 1)
                self.assertIn("env cannot use", errors[0])

    def test_job_env_rejects_runner_but_allows_job_planning_contexts(self):
        source = "jobs:\n  test:\n    env:\n      VALUE: ${{ %s }}\n    steps:\n      - run: true\n"
        for expression in ("runner.temp", "steps.build.outputs.path", "job.status", "env.OTHER"):
            with self.subTest(expression=expression):
                self.assertEqual(len(checker.check(source % expression)), 1)
        for expression in ("matrix.target", "needs.build.outputs.version", "strategy.job-index",
                           "github.sha", "vars.TARGET", "inputs.target", "secrets.TOKEN"):
            with self.subTest(expression=expression):
                self.assertEqual(checker.check(source % expression), [])

    def test_step_env_run_and_action_inputs_allow_runner_context(self):
        source = """jobs:
  test:
    steps:
      - name: Runner context
        env:
          VALUE: ${{ runner.temp }}
        run: echo "$VALUE"
      - uses: actions/cache@sha
        with:
          key: ${{ runner.os }}
"""
        self.assertEqual(checker.check(source), [])

    def test_inline_env_and_multiline_expressions_are_checked(self):
        self.assertEqual(len(checker.check("env: {VALUE: '${{ runner.temp }}'}\n")), 1)
        source = "jobs:\n  test:\n    env:\n      VALUE: >-\n        ${{\n          runner.temp\n        }}\n"
        self.assertEqual(len(checker.check(source)), 1)

    def test_literals_and_properties_are_not_mistaken_for_context_roots(self):
        source = """# ${{ runner.temp }}
env:
  VALUE: ${{ format('runner.temp {0}', github.runner) }}
  REF: ${{ github['runner'] }}
  LITERAL: runner.temp
  BRANCH: ${{ github.ref }} # runner.temp
"""
        self.assertEqual(checker.check(source), [])

    def test_hash_files_is_not_available_in_early_env(self):
        self.assertIn("cannot use hashFiles", checker.check("env:\n  HASH: ${{ hashFiles('Cargo.lock') }}\n")[0])


class CargoHome(unittest.TestCase):
    def test_initialization_creates_unique_empty_homes_and_preserves_environment_file(self):
        with tempfile.TemporaryDirectory(prefix="runner temp ") as temporary:
            root = Path(temporary)
            environment_file = root / "github env"
            environment_file.write_text("EXISTING=value\n")
            environment = dict(os.environ, RUNNER_TEMP=str(root), GITHUB_ENV=str(environment_file))
            for _ in range(2):
                subprocess.run(["bash", str(SCRIPTS / "initialize-cargo-home.sh")],
                               env=environment, check=True, capture_output=True, text=True)
            lines = environment_file.read_text().splitlines()
            self.assertEqual(lines[0], "EXISTING=value")
            homes = [Path(line.removeprefix("CARGO_HOME=")) for line in lines[1:]]
            self.assertEqual(len(set(homes)), 2)
            for home in homes:
                self.assertEqual(home.parent, root)
                self.assertTrue(home.is_dir())
                self.assertEqual(list(home.iterdir()), [])

    def test_initialization_requires_runner_environment(self):
        for missing in ("RUNNER_TEMP", "GITHUB_ENV"):
            with self.subTest(missing=missing):
                environment = dict(os.environ, RUNNER_TEMP="/tmp", GITHUB_ENV="/dev/null")
                del environment[missing]
                result = subprocess.run(["bash", str(SCRIPTS / "initialize-cargo-home.sh")],
                                        env=environment, check=False, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(missing, result.stderr)


if __name__ == "__main__":
    unittest.main()
