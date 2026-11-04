// A real directory response is the edge oracle. No time ordering or branch
// composition references may manufacture an ancestry connector.
const probe = window.__zxsDirectoryHistory;
const panel = document.querySelector('.zxs-directory-history');
if (!probe?.nodes || !panel || panel.getAttribute('aria-busy') !== 'false') return false;
const expected = probe.nodes.nodes
    .filter(node => node.parent_endpoint === 'in_window')
    .map(node => [node.parent_tree_id, node.tree_id]);
if (expected.length === 0) throw new Error('directory fixture has no real in-window parent edges');
const svg = panel.querySelector('svg.zxs-version-tree-links');
const paths = Array.from(panel.querySelectorAll('path[data-parent-tree][data-child-tree]'));
const actual = paths.map(path => [path.dataset.parentTree, path.dataset.childTree]);
const ordered = pairs => pairs.map(pair => JSON.stringify(pair)).sort();
if (JSON.stringify(ordered(actual)) !== JSON.stringify(ordered(expected))) return false;
const svgRect = svg?.getBoundingClientRect();
const matrix = svg?.getScreenCTM();
const svgStyle = svg && getComputedStyle(svg);
if (!matrix || svgRect.width <= 0 || svgRect.height <= 0 || Number(svgStyle.opacity) <= 0) return false;
const close = (actual, expected) => Math.abs(actual - expected) <= 2;
for (const path of paths) {
    const style = getComputedStyle(path);
    const d = path.getAttribute('d');
    if (!d || /NaN|Infinity/.test(d) || style.display === 'none' || style.visibility === 'hidden'
        || style.stroke === 'none' || !Number.isFinite(parseFloat(style.strokeWidth)) || parseFloat(style.strokeWidth) <= 0
        || Number(style.opacity) <= 0 || Number(style.strokeOpacity) <= 0
        || !path.getAttribute('marker-end')?.includes('#zxs-directory-arrow')) return false;
    const length = path.getTotalLength();
    if (!Number.isFinite(length) || length <= 0) return false;
    const parent = panel.querySelector('[data-directory-tree-id="' + path.dataset.parentTree + '"]');
    const child = panel.querySelector('[data-directory-tree-id="' + path.dataset.childTree + '"]');
    if (!parent || !child) return false;
    const source = parent.getBoundingClientRect(), target = child.getBoundingClientRect();
    if (source.top <= target.bottom) return false;
    const start = path.getPointAtLength(0).matrixTransform(matrix);
    const end = path.getPointAtLength(length).matrixTransform(matrix);
    if (!close(start.x, (source.left + source.right) / 2) || !close(start.y, source.top)
        || !close(end.x, (target.left + target.right) / 2) || !close(end.y, target.bottom + 3)) return false;
}
return true;
