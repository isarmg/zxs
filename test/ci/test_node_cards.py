"""Card source/palette and probe checks, not production-browser acceptance."""
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
STYLES = ROOT / "src/web/public/styles"


def luminance(hex_color):
    values = [int(hex_color[index:index + 2], 16) / 255 for index in (1, 3, 5)]
    linear = [value / 12.92 if value <= .04045 else ((value + .055) / 1.055) ** 2.4
              for value in values]
    return sum(value * weight for value, weight in zip(linear, [.2126, .7152, .0722]))


class NodeCards(unittest.TestCase):
    def test_probe_positive_and_negative_host_cases(self):
        subprocess.run(["node", "--test", "test/tools/browser-smoke/unit/node_cards_probe_test.mjs"],
                       cwd=ROOT, check=True)

    def test_palette_is_distinct_and_has_graphic_contrast_in_both_themes(self):
        for filename, background in [("tokens.css", "#ffffff"), ("tokens.dark.css", "#18181b")]:
            values = dict(re.findall(r"--zxs-history-([a-z]+): (#[0-9a-f]{6});",
                                     (STYLES / "tokens" / filename).read_text()))
            self.assertEqual(set(values), {"content", "rename", "move", "path", "tags", "merge", "snapshot", "delete"})
            self.assertEqual(len(set(values.values())), len(values))
            for name, color in values.items():
                light, dark = sorted([luminance(color), luminance(background)], reverse=True)
                self.assertGreaterEqual((light + .05) / (dark + .05), 3, (filename, name))

    def test_actions_have_distinct_stroke_shapes_without_relying_on_color(self):
        source = (ROOT / "src/web/src/icons.rs").read_text()
        paths = dict(re.findall(r'"(content|rename|move|path-change|tags|merge|snapshot|delete)" => "([^"]+)"', source))
        self.assertEqual(len(paths), 8)
        self.assertEqual(len(set(paths.values())), len(paths))

    def test_two_rows_native_disclosure_and_unchanged_control_ownership_are_wired(self):
        css = (STYLES / "components/file-manager.css").read_text()
        self.assertIn("--zxs-history-card-width: 10ch", css)
        self.assertIn("grid-template-rows: repeat(2,var(--zxs-compact-row-height))", css)
        self.assertIn(".zxs-node-card:focus-visible", css)
        card = (ROOT / "src/web/src/files/node_card.rs").read_text()
        self.assertIn('<summary class="zxs-node-card zxs-frame"', card)
        self.assertIn('role="img" aria-label=action.label() title=action.label()', card)
        self.assertNotIn("Action::Delete", card)
        for filename in ["version_tree.rs", "directory_history/view.rs"]:
            source = (ROOT / "src/web/src/files" / filename).read_text()
            self.assertIn('<details class="zxs-node-disclosure">', source)
            self.assertIn("node_card::summary(", source)
        for runner in ("tree_frames.rs", "directory_history.rs"):
            tree = (ROOT / "test/tools/browser-smoke/runner" / runner).read_text()
            self.assertLess(tree.index("node_cards::open_first(browser)?;"),
                            tree.index('include_str!("history_title_rows.js")'))
        self.assertIn("node_cards::open_current(browser)?;",
                      (ROOT / "test/tools/browser-smoke/runner/phase1_files.rs").read_text())

    def test_current_card_uses_supported_outer_paint_without_changing_layout(self):
        css = (STYLES / "components/file-manager.css").read_text()
        current = re.search(r"\.zxs-node-card\[aria-current=true\]\s*\{([^}]+)\}", css).group(1)
        self.assertEqual(current.strip(), "--zxs-contour-mode: paint; box-shadow: 0 0 0 2px var(--zxs-text-link);")
        probe = (ROOT / "test/tools/browser-smoke/runner/node_cards.js").read_text()
        self.assertIn("getAttribute('data-zxs-contour-state') !== (current ? 'paint' : 'clip')", probe)
        self.assertIn("current ? style.maskImage !== 'none'", probe)
        for property in ("borderImageSource", "borderImageSlice", "maskImage", "borderImageOutset"):
            self.assertIn(property, probe)
        runner = (ROOT / "test/tools/browser-smoke/runner/node_cards.rs").read_text()
        self.assertIn('for theme in ["light", "dark"]', runner)
        self.assertIn("accessibility::select_theme(browser, theme)", runner)
        self.assertIn("p.card.matches(':focus-visible')", runner)

    def test_expected_state_follows_the_actual_shared_frame_mode(self):
        geometry = (STYLES / "components/geometry.css").read_text()
        frame = re.search(r"\.zxs-app-shell \.zxs-frame\s*\{([^}]+)\}", geometry).group(1)
        self.assertIn("--zxs-contour-mode: clip;", frame)
        controller = (ROOT / "src/interface/src/contours/controller.rs").read_text()
        self.assertIn('let clipping = mode == "clip"', controller)
        self.assertIn('record.mark(if clipping { "clip" } else { "paint" }, None)', controller)
        runner = (ROOT / "test/tools/browser-smoke/runner/node_cards.rs").read_text()
        self.assertIn("p.card.getAttribute('aria-current')==='true'?'paint':'clip'", runner)


if __name__ == "__main__":
    unittest.main()
