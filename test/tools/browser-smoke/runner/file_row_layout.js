// Observe native table geometry, inset separators and the real shared G4 output.
// This probe does not change product DOM, selection, focus or pointer state.
const panel = document.querySelector('.zxs-file-list-panel');
const table = panel.querySelector('.zxs-file-table');
const scroll = table.parentElement;
const nav = panel.querySelector('.zxs-file-pagination');
const extent = parseFloat(getComputedStyle(panel).getPropertyValue('--zxs-contour-extent'));
const body = [...table.tBodies[0].rows];
const heads = [...table.tHead.rows];
const close = (a, b) => Number.isFinite(a) && Number.isFinite(b) && Math.abs(a - b) <= .5;
const transparent = color => color === 'transparent' || color === 'rgba(0, 0, 0, 0)';
const color = value => {
    const canvas = document.createElement('canvas'); canvas.width = canvas.height = 1;
    const context = canvas.getContext('2d'); context.fillStyle = value; context.fillRect(0, 0, 1, 1);
    return [...context.getImageData(0, 0, 1, 1).data].join(',');
};
const scrollBox = scroll.getBoundingClientRect();
const viewport = {left: scrollBox.left + scroll.clientLeft, width: scroll.clientWidth};
const line = (node, pseudo, expected) => {
    const css = getComputedStyle(node, pseudo);
    const present = css.content !== 'none' && css.content !== 'normal';
    if (!expected) return {valid: !present, present};
    const width = parseFloat(css.width), box = node.getBoundingClientRect();
    const bounds = pseudo === '::before' ? box : viewport;
    const start = box.left + parseFloat(css.left), end = start + width;
    const left = start - bounds.left, right = bounds.left + bounds.width - end;
    const expectedColor = color(css.getPropertyValue('--zxs-block-border').trim());
    return {valid: present && extent > 0 && close(left, extent) && close(right, extent)
        && close(width, bounds.width - extent * 2)
        && parseFloat(css.borderBottomWidth) === 1 && css.borderBottomStyle === 'solid'
        && color(css.borderBottomColor) === expectedColor && expectedColor !== '0,0,0,0'
        && css.pointerEvents === 'none', present, left, right, width};
};
const decoration = row => row.querySelector(':scope > :is(th,td) > .zxs-file-row-decoration');
const headerLines = heads.map(row => line(decoration(row), '::after', body.length > 0));
const headerBorders = heads.every(row => [...row.cells].every(cell => parseFloat(getComputedStyle(cell).borderBottomWidth) === 0));
const hoverEnabled = matchMedia('(hover: hover) and (pointer: fine)').matches;
const rows = body.map((row, index) => {
    const wrapper = decoration(row);
    const surface = row.querySelector(':scope > td > .zxs-file-row-decoration > .zxs-file-row-surface');
    const separator = line(wrapper, '::after', index < body.length - 1);
    if (!surface) return {valid: false, index, reason: 'missing row surface', separator};
    const css = getComputedStyle(surface), rowCss = getComputedStyle(row);
    const box = row.getBoundingClientRect(), fill = surface.getBoundingClientRect();
    const expectedOpacity = row.getAttribute('aria-selected') === 'true' || (hoverEnabled && row.matches(':hover')) ? 1 : 0;
    const bounds = close(fill.left, viewport.left) && close(fill.width, viewport.width)
        && close(fill.top, box.top) && close(fill.height, box.height);
    const paint = surface.getAttribute('data-zxs-contour-state') === 'paint'
        && css.borderImageSource.startsWith('url(') && css.borderImageSlice.includes('fill')
        && close(parseFloat(css.getPropertyValue('--zxs-contour-extent')), extent);
    const decorationOnly = getComputedStyle(wrapper).overflowX === 'clip'
        && getComputedStyle(wrapper).overflowY === 'clip' && wrapper.contains(surface)
        && wrapper.querySelectorAll('button,input,select,textarea,[tabindex]').length === 0;
    const interaction = decorationOnly && surface.getAttribute('aria-hidden') === 'true' && surface.tabIndex === -1
        && css.pointerEvents === 'none' && Number(css.opacity) === expectedOpacity
        && rowCss.display === 'table-row' && transparent(rowCss.backgroundColor)
        && rowCss.clipPath === 'none' && rowCss.maskImage === 'none';
    const cellBorders = [...row.cells].every(cell => parseFloat(getComputedStyle(cell).borderBottomWidth) === 0);
    return {valid: separator.valid && bounds && paint && interaction && cellBorders,
        index, separator, bounds, paint, interaction, cellBorders, expectedOpacity};
});
const pagination = nav ? line(nav, '::before', true) : {valid: true, absent: true};
const joined = !nav || close(nav.getBoundingClientRect().top, scroll.getBoundingClientRect().bottom);
const documentFits = document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1;
return {valid: extent > 0 && headerLines.every(entry => entry.valid) && headerBorders && rows.every(row => row.valid)
    && pagination.valid && joined && documentFits, extent, headerLines, headerBorders, rows, pagination, joined, documentFits, scrollLeft: scroll.scrollLeft, viewport};
