(async () => {
    const p = window.__zxsTreeFrames;
    await p.settle();
    const root = p.current(), scroll = root.parentElement, rect = root.getBoundingClientRect();
    const nodes = [...root.querySelectorAll('.zxs-version-node')];
    const boxes = new Map(nodes.map(node => [node.dataset.versionId, node.getBoundingClientRect()]));
    const edges = [...root.querySelectorAll('.zxs-version-tree-edge')];
    p.check(nodes.length >= 2 && edges.length >= 1, 'real seeded nodes and connectors');
    p.check(p.roots.get(root).initial > 0, 'first measure synchronous before observer registration');
    p.check(document.documentElement.scrollWidth <= innerWidth + 1, 'tree must not overflow page');
    for (const node of nodes) {
        const box = node.getBoundingClientRect(), style = getComputedStyle(node);
        p.check(box.width <= 295 && box.width <= scroll.clientWidth - 35, 'node width and side channels');
        p.check(parseFloat(style.getPropertyValue('--zxs-contour-extent')) === 18, 'G4 node extent');
        p.check(Math.abs(parseFloat(style.fontSize) - 14.4) < .1, 'unchanged compact font');
    }
    for (const edge of edges) {
        const parent = boxes.get(edge.dataset.parentVersion), child = boxes.get(edge.dataset.childVersion);
        const length = edge.getTotalLength(), start = edge.getPointAtLength(0), end = edge.getPointAtLength(length);
        const near = (a,b) => Math.abs(a-b) < .1;
        p.check(parent.top > child.bottom, 'parent below child');
        p.check(near(start.x,(parent.left+parent.right)/2-rect.left) && near(start.y,parent.top-rect.top), 'parent connector endpoint');
        p.check(near(end.x,(child.left+child.right)/2-rect.left) && near(end.y,child.bottom+3-rect.top), 'child connector endpoint');
        const style = getComputedStyle(edge);
        p.check(style.strokeWidth === '1.5px' && style.strokeLinecap === 'round' && style.strokeLinejoin === 'round', 'connector stroke');
        p.check(edge.getAttribute('marker-end') === 'url(#zxs-version-arrow)', 'upward arrow retained');
        for (let d = .75; d < length; d += .75) {
            const point = edge.getPointAtLength(d), x = point.x+rect.left, y = point.y+rect.top;
            for (const box of boxes.values()) p.check(!(x > box.left+.1 && x < box.right-.1 && y > box.top+.1 && y < box.bottom-.1), 'connector avoids node interiors');
        }
    }
    return {nodes:nodes.length, edges:edges.length, width:innerWidth, fonts:document.fonts.status};
})()
