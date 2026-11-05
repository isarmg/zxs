"""Source contracts only; the browser fixture measures actual page geometry."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class PageRows(unittest.TestCase):
    def test_status_text_panels_compose_shared_rows_not_ad_hoc_padding(self):
        for path in ['src/web/src/monitor.rs', 'src/web/src/monitor/scrub.rs']:
            source = (ROOT / path).read_text()
            self.assertIn('class="zxs-content-panel zxs-frame zxs-stack"', source)
            self.assertIn('<h2 class="zxs-row">', source)
            self.assertNotIn('<h2>', source)
            self.assertNotIn('<p>', source)
            self.assertNotIn('<p role=', source)

    def test_pagination_joins_borderless_rows_even_with_no_body_rows(self):
        css = (ROOT / 'src/web/public/styles/components/file-manager.css').read_text()
        self.assertNotIn('.zxs-file-pagination::before', css)
        self.assertNotIn('.zxs-file-row-decoration::after', css)
        self.assertNotIn('border-block-start:', css)
        view = (ROOT / 'src/web/src/files/view.rs').read_text()
        self.assertIn('<nav class="zxs-file-pagination zxs-flow"', view)
        probe = (ROOT / 'test/tools/browser-smoke/runner/file_row_layout.js').read_text()
        self.assertIn("const pagination = nav ? noLine(nav, '::before')", probe)
        self.assertIn('close(nav.getBoundingClientRect().top, scroll.getBoundingClientRect().bottom)', probe)
        self.assertIn('pagination.valid && joined && documentFits', probe)
        page = (ROOT / 'test/tools/browser-smoke/runner/page_layout.rs').read_text()
        self.assertIn('for empty in [false, true]', page)
        self.assertIn('file_row_geometry(browser)?;', page)

    def test_disk_id_label_changes_without_changing_the_identity_value(self):
        source = (ROOT / 'src/web/src/admin/disks.rs').read_text()
        self.assertIn('("zxs ID".to_owned(), record.disk_id.clone())', source)
        self.assertNotIn('ZXS 登记磁盘 ID', source)
        self.assertNotIn('ZXS registered disk ID', source)
        self.assertIn('record.filesystem_uuid.as_deref()', source)
