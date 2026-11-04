// Read-only observation of the exact native pointer target. A null point asks
// for the current visible center; a supplied point observes where we just moved.
const index = arguments[0], row = document.querySelectorAll('.zxs-file-table tbody tr')[index];
const scroll = row.closest('.zxs-table-scroll'), r = row.getBoundingClientRect(), b = scroll.getBoundingClientRect();
const left = Math.max(0, b.left + scroll.clientLeft, r.left);
const right = Math.min(innerWidth, b.left + scroll.clientLeft + scroll.clientWidth, r.right);
const point = arguments[1] ?? {x: Math.floor((left + right) / 2), y: Math.floor(r.top + r.height / 2)};
const hit = document.elementFromPoint(point.x, point.y);
const hitOnRow = hit === row || row.contains(hit);
const visible = right > left && r.top >= 0 && r.bottom <= innerHeight
    && point.x >= left && point.x < right && point.y >= r.top && point.y < r.bottom;
const hoverEnabled = matchMedia('(hover: hover) and (pointer: fine)').matches;
const device = {hover: matchMedia('(hover: hover)').matches, anyHover: matchMedia('(any-hover: hover)').matches,
    fine: matchMedia('(pointer: fine)').matches, anyFine: matchMedia('(any-pointer: fine)').matches,
    coarse: matchMedia('(pointer: coarse)').matches, none: matchMedia('(pointer: none)').matches,
    maxTouchPoints: navigator.maxTouchPoints};
const hovered = row.matches(':hover'), surface = row.querySelector('.zxs-file-row-surface'), css = getComputedStyle(surface);
return {valid: visible && hitOnRow && hoverEnabled && hovered && css.opacity === '1',
    index, kind: row.querySelector('.zxs-file-icon--folder') ? 'folder' : 'file', theme: document.documentElement.dataset.zxsTheme,
    point, visible, hitOnRow, hoverEnabled, device, hovered, opacity: css.opacity, contour: surface.dataset.zxsContourState,
    hit: hit ? {tag: hit.tagName, className: hit.getAttribute('class'), text: hit.textContent.slice(0, 120)} : null,
    row: {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height},
    viewport: {width: innerWidth, height: innerHeight, scrollX, scrollY},
    scroll: {left: scroll.scrollLeft, width: scroll.clientWidth, cssLeft: getComputedStyle(scroll).getPropertyValue('--zxs-file-scroll-left')},
    surface: {pointerEvents: css.pointerEvents, backgroundColor: css.backgroundColor, borderImageSource: css.borderImageSource}};
