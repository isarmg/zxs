"""Keep post-packaging acceptance bound to extracted release artifacts."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ReleaseAcceptance(unittest.TestCase):
    def test_release_checks_extracted_archives_before_upload(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        self.assertLess(workflow.index('release-package.sh'), workflow.index('release-acceptance.sh'))
        release_upload = workflow.rindex('actions/upload-artifact', 0, workflow.index('name: release-linux-web'))
        self.assertLess(workflow.index('release-acceptance.sh'), release_upload)
        script = (ROOT / '.github/scripts/release-acceptance.sh').read_text()
        self.assertIn('cp "release-assets/zxs-${RELEASE_VERSION}-debian13-amd64"', script)
        self.assertIn('--cli-bin "$work/server/zxs" --embedded-web', script)
        self.assertIn('cargo run --locked --offline --release -p browser-smoke', script)
        self.assertNotIn('--web-dir', script)
        self.assertNotIn('target/native/debug/zxs', script)
        self.assertIn('wc -l)" = 1', script)

    def test_main_ci_accepts_extracted_archives_without_duplicate_browser_suite(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        browser = workflow.split('  browser-smoke:\n', 1)[1].split('  smb-sdk:', 1)[0]
        self.assertIn('name: web-dist', browser)
        self.assertIn('python3 .github/scripts/release-native.py', browser)
        self.assertLess(browser.index('release-package.sh'), browser.index('release-acceptance.sh'))
        self.assertIn('check-csr-browser.py', browser)
        self.assertNotIn('check.sh browser-smoke', browser)
        self.assertIn('ZXS_SMOKE_SCREENSHOT:', browser)

    def test_main_ci_retains_complete_candidate_only_after_archive_acceptance(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        browser = workflow.split('  browser-smoke:\n', 1)[1].split('  smb-sdk:', 1)[0]
        self.assertLess(browser.index('release-acceptance.sh'), browser.index('release-assemble.sh'))
        self.assertLess(browser.index('check-csr-browser.py'), browser.index('release-assemble.sh'))
        self.assertLess(browser.index('release-assemble.sh'), browser.index('installation-candidate-linux-web'))
        candidate = browser.split('      - name: Retain verified installation candidate', 1)[1].split('      - name:', 1)[0]
        self.assertIn('path: release-assets/*', candidate)
        self.assertIn('if-no-files-found: error', candidate)
        self.assertIn('retention-days: 7', candidate)
        self.assertNotIn('if:', candidate)
        self.assertNotIn('release-publish.sh', workflow)
        self.assertNotIn('contents: write', workflow)
        for path in ('docs/ci.md', 'docs/release.md'):
            self.assertIn('installation-candidate-linux-web', (ROOT / path).read_text())

    def test_release_rebuilds_both_native_products_before_packaging(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        build = 'python3 .github/scripts/release-native.py'
        self.assertLess(workflow.index(build), workflow.index('release-package.sh'))
        documentation = (ROOT / 'docs/release.md').read_text()
        self.assertIn('不保证完全可重现', documentation)
        self.assertIn('修改源码后不能沿用旧', documentation)

    def test_installation_recipe_has_one_authoritative_source(self):
        release = (ROOT / 'docs/release.md').read_text()
        install = (ROOT / 'docs/templates/release-install.md').read_text()
        self.assertIn('(templates/release-install.md)', release)
        self.assertNotIn('sha256sum ', release)
        self.assertIn('set -eu\nVERSION="$(python3', install)
        self.assertLess(install.index('if names != expected:'), install.index('sha256sum --check SHA256SUMS'))
        self.assertLess(install.index('sha256sum --check SHA256SUMS'), install.index('mkdir "server-$VERSION"'))
        self.assertIn('不是独立的发布者签名', install)
        self.assertIn('`public/`', install)
        self.assertTrue((ROOT / 'test/ci/release-publish.sh').is_file())
        self.assertNotIn('.github/scripts/tests/', release)


if __name__ == '__main__':
    unittest.main()
