// Observe borderless native table geometry and the real shared G4 row output.
// This probe does not change product DOM, selection, focus or pointer state.
const panel = document.querySelector('.zxs-file-list-panel');
const table = panel.querySelector('.zxs-file-table');
const scroll = table.parentElement;
const nav = panel.querySelector('.zxs-file-pagination');
const zoom = scroll.currentCSSZoom;
const extent = parseFloat(getComputedStyle(panel).getPropertyValue('--zxs-contour-extent'));
const body = [...table.tBodies[0].rows];
const heads = [...table.tHead.rows];
const close = (a, b) => Number.isFinite(a) && Number.isFinite(b) && Math.abs(a - b) <= .5;
const transparent = color => color === 'transparent' || color === 'rgba(0, 0, 0, 0)';
const scrollBox = scroll.getBoundingClientRect();
// This borderless, auto-height scrollport has no vertical scrollbar. Its rect
// retains fractional width; integer clientWidth would lose precision under zoom.
const scrollport = scroll.clientLeft === 0 && scroll.offsetWidth === scroll.clientWidth;
const viewport = {left: scrollBox.left, width: scrollBox.width};
const noLine = (node, pseudo) => {
    const css = getComputedStyle(node, pseudo);
    const present = css.content !== 'none' && css.content !== 'normal';
    return {valid: !present, present};
};
const decoration = row => row.querySelector(':scope > :is(th,td) > .zxs-file-row-decoration');
const headerLines = heads.map(row => noLine(decoration(row), '::after'));
const headerBorders = heads.every(row => [...row.cells].every(cell => parseFloat(getComputedStyle(cell).borderBottomWidth) === 0));
const hoverEnabled = matchMedia('(hover: hover) and (pointer: fine)').matches;
const rows = body.map((row, index) => {
    const wrapper = decoration(row);
    const surface = row.querySelector(':scope > td > .zxs-file-row-decoration > .zxs-file-row-surface');
    const separator = noLine(wrapper, '::after');
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
const pagination = nav ? noLine(nav, '::before') : {valid: true, absent: true};
const joined = !nav || close(nav.getBoundingClientRect().top, scroll.getBoundingClientRect().bottom);
const documentFits = document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1;
return {valid: Number.isFinite(zoom) && zoom > 0 && scrollport && extent > 0 && headerLines.every(entry => entry.valid) && headerBorders && rows.every(row => row.valid)
    && pagination.valid && joined && documentFits, zoom, scrollport, extent, headerLines, headerBorders, rows, pagination, joined, documentFits, scrollLeft: scroll.scrollLeft, viewport};
