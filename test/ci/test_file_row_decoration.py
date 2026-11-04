"""Source and probe-model contracts, not actual browser rendering acceptance."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class FileRowDecoration(unittest.TestCase):
    def test_probe_and_model_cases(self):
        subprocess.run(['node', '--check', 'test/tools/browser-smoke/runner/file_row_layout.js'],
                       cwd=ROOT, check=True)
        subprocess.run(['node', 'test/tools/browser-smoke/unit/file_row_layout_probe_test.mjs'],
                       cwd=ROOT, check=True)

    def test_screenshot_pixel_probe_and_negative_controls(self):
        subprocess.run(['node', '--check', 'test/tools/browser-smoke/runner/file_row_pixels.js'],
                       cwd=ROOT, check=True)
        subprocess.run(['node', 'test/tools/browser-smoke/unit/file_row_pixels_probe_test.mjs'],
                       cwd=ROOT, check=True)

    def test_native_pointer_observer_and_pre_cleanup_evidence(self):
        subprocess.run(['node', '--check', 'test/tools/browser-smoke/runner/file_row_pointer.js'],
                       cwd=ROOT, check=True)
        subprocess.run(['node', 'test/tools/browser-smoke/unit/file_row_pointer_probe_test.mjs'],
                       cwd=ROOT, check=True)
        visual = (ROOT / 'test/tools/browser-smoke/runner/file_row_visual.rs').read_text()
        self.assertIn('"pointerType":"mouse"', visual)
        self.assertIn('"origin":"viewport"', visual)
        self.assertIn('requestAnimationFrame(()=>requestAnimationFrame(', visual)
        self.assertIn('pointer_state(browser, index, point)?', visual)
        self.assertIn('let evidence = pointer_evidence(browser, &state, end, result.is_err());', visual)
        self.assertLess(visual.index('pointer_evidence(browser, &state, end, result.is_err())'),
                        visual.index('if let Err(error) = result'))
        self.assertIn('"captureBeyondViewport":false', visual)
        self.assertNotIn('CSS.forcePseudoState', visual)

    def test_inset_lines_and_g4_fill_reuse_the_frame_corner(self):
        css = (ROOT / 'src/web/public/styles/components/file-manager.css').read_text()
        self.assertIn('inset-inline: var(--zxs-contour-panel);', css)
        surface = css.split('.zxs-file-row-surface {', 1)[1].split('}', 1)[0]
        for declaration in ['position: absolute;', 'inset-block: 0;',
                            'left: var(--zxs-file-scroll-left);', 'width: 100cqw;',
                            'pointer-events: none;', 'opacity: 0;',
                            '--zxs-contour-extent: var(--zxs-contour-panel);']:
            self.assertIn(declaration, surface)
        self.assertIn('container-type: inline-size;', css)
        self.assertIn('left: calc(var(--zxs-file-scroll-left) + var(--zxs-contour-panel));', css)
        self.assertIn('width: calc(100cqw - var(--zxs-contour-panel) * 2);', css)
        decoration = css.split('.zxs-file-row-decoration {', 1)[1].split('}', 1)[0]
        for declaration in ['position: absolute;', 'inset: 0;', 'overflow: clip;',
                            'z-index: -1;', 'pointer-events: none;']:
            self.assertIn(declaration, decoration)
        self.assertNotIn('border-radius:', css)
        self.assertNotIn('clip-path:', css)
        self.assertIn('.zxs-file-table tr { position: relative; isolation: isolate; }', css)
        self.assertIn('.zxs-file-table tr[aria-selected=true] > td > .zxs-file-row-decoration > .zxs-file-row-surface { opacity: 1; }', css)
        self.assertIn('@media (hover: hover) and (pointer: fine)', css)
        self.assertIn('.zxs-file-table tbody tr:hover > td > .zxs-file-row-decoration > .zxs-file-row-surface { opacity: 1; }', css)
        self.assertNotIn('tr:hover { background:', css)

    def test_decoration_does_not_replace_native_rows_or_actions(self):
        view = (ROOT / 'src/web/src/files/view.rs').read_text()
        self.assertEqual(view.count('<span class="zxs-file-row-surface" aria-hidden="true"></span>'), 2)
        self.assertEqual(view.count('web_interface::ev::on(web_interface::ev::Custom::new("scroll"), sync_row_viewport)'), 2)
        self.assertNotIn('on:scroll=', view)
        self.assertIn('js_sys::Reflect::get(scroll.as_ref(), &"scrollLeft".into())', view)
        self.assertNotIn('scroll.scroll_left()', view)
        self.assertIn('<tr class:is-selected=move ||checked.get()', view)
        self.assertIn('aria-selected=move ||checked.get().to_string()', view)
        self.assertIn('on:click=move |_|activate_row(state,row,false)', view)
        self.assertIn('event.stop_propagation();activate_row(state,row,true)', view)
        self.assertIn('class="zxs-entry-checkbox" type="checkbox"', view)
        self.assertIn('on:click=move |event|event.stop_propagation()', view)
        self.assertIn('actions::change_page(state,false)', view)
        self.assertIn('actions::change_page(state,true)', view)

    def test_browser_cases_use_real_rows_at_multiple_viewports(self):
        page = (ROOT / 'test/tools/browser-smoke/runner/page_layout.rs').read_text()
        pagination = (ROOT / 'test/tools/browser-smoke/runner/file_pagination.rs').read_text()
        rows = (ROOT / 'test/tools/browser-smoke/runner/file_rows.rs').read_text()
        self.assertIn('include_str!("file_row_layout.js")', page)
        self.assertIn('for empty in [false, true]', page)
        self.assertIn('(320, 568)', page)
        self.assertIn('page_layout::file_row_sizes(browser)?;', pagination)
        self.assertIn('pointer hover shows the shared rounded row surface', rows)
        self.assertIn('leaving an unchecked row clears its hover', rows)
        self.assertIn('rowSurfaceShown:', rows)
        visual = (ROOT / 'test/tools/browser-smoke/runner/file_row_visual.rs').read_text()
        main = (ROOT / 'test/tools/browser-smoke/runner/main.rs').read_text()
        self.assertIn('for end in [false, true, false]', page)
        self.assertIn('file_row_resize(browser)?;', page)
        self.assertIn('right-scrolled list expands without retained overflow or blank space', page)
        self.assertIn('deviceScaleFactor":2', page)
        self.assertIn('!Number.isInteger(s.scrollLeft)', page)
        self.assertIn('Math.abs(offset-s.scrollLeft)<.01', page)
        self.assertIn('before_page = page_layout::file_row_shrink_start(browser)?;', pagination)
        self.assertIn('file_row_shrink_finish(browser, before_page, "pagination")?;', pagination)
        self.assertIn('before_query = page_layout::file_row_shrink_start(browser)?;', pagination)
        self.assertIn('file_row_shrink_finish(browser, before_query, "search")?;', pagination)
        self.assertIn("size:id==='fixture-199'?'1124774006935781376':base.size", pagination)
        finish = page.split('pub(super) fn file_row_shrink_finish(', 1)[1].split('fn file_row_resize(', 1)[0]
        self.assertNotIn('s.scrollLeft=', finish)
        self.assertIn('s.scrollWidth<=Math.ceil(width)+1', finish)
        self.assertIn('width<{}-1', finish)
        self.assertIn('s.scrollWidth-s.clientWidth', page)
        self.assertIn('file_row_visual::run(browser, &[0, 2, 4], "file")?;', pagination)
        self.assertIn('file_row_visual::run(browser, &[0], "folder")?;', main)
        self.assertIn('"Page.captureScreenshot"', visual)
        self.assertIn('include_str!("file_row_pixels.js")', visual)
        self.assertLess(visual.index('directory.join(format!("{label}.png"))'), visual.index('if result["valid"] != true'))
        self.assertIn('focus_pixels(browser, ".zxs-entry-checkbox")?;', visual)
        self.assertIn('focus_pixels(browser, ".zxs-entry-actions .zxs-button")?;', visual)
