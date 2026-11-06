// Read-only presentation assertion against production DOM and loaded fonts.
if (document.fonts.status !== 'loaded') return false;
const cards = [...document.querySelectorAll('.zxs-node-card')];
if (!cards.length || !cards.some(card => card.getAttribute('aria-current') === 'true')) return false;
const close = (a, b) => Number.isFinite(a) && Number.isFinite(b) && Math.abs(a - b) <= .75;
for (const card of cards) {
    if (card.tagName !== 'SUMMARY' || card.parentElement.tagName !== 'DETAILS') return false;
    const title = card.querySelector(':scope > .zxs-node-card-title');
    const row = card.querySelector(':scope > .zxs-node-card-actions');
    if (!title || !row) return false;
    const icons = [...row.querySelectorAll(':scope > .zxs-node-action')];
    const content = [...card.children].filter(child => !child.matches('svg[data-zxs-contour-decoration="foreground"]'));
    if (content.length !== 2 || !title.textContent || !icons.length
        || row.textContent.trim() || card.title !== title.textContent || !card.getAttribute('aria-label')) return false;
    const style = getComputedStyle(card), titleStyle = getComputedStyle(title);
    // Both states need real G4 output. Only the current card opts out of the
    // original contour mask, which would clip the outer ring's corner arcs.
    const current = card.getAttribute('aria-current') === 'true';
    if (!style.borderImageSource.startsWith('url(') || !style.borderImageSlice.includes('fill')) return false;
    if (card.getAttribute('data-zxs-contour-state') !== (current ? 'paint' : 'clip')
        || (current ? style.maskImage !== 'none' : !style.maskImage.startsWith('url('))) return false;
    // A 2px outer current-state ring needs the G4 paint's 2px safety margin too.
    const outset = parseFloat(style.borderImageOutset);
    if (current && (!Number.isFinite(outset) || outset < 4)) return false;
    const box = card.getBoundingClientRect(), rowBox = row.getBoundingClientRect();
    const size = parseFloat(style.getPropertyValue('--zxs-compact-row-height'));
    const zoom = card.currentCSSZoom;
    const canvas = document.createElement('canvas'), context = canvas.getContext('2d');
    context.font = `${style.fontSize} ${style.fontFamily}`;
    const width = context.measureText('0000000000').width;
    if (!close(box.width, width * zoom) || !close(box.height, 2 * size * zoom)
        || !close(rowBox.height, size * zoom) || !close(rowBox.top, box.top + size * zoom)
        || titleStyle.textOverflow !== 'ellipsis' || titleStyle.whiteSpace !== 'nowrap') return false;
    const colors = new Set();
    for (const icon of icons) {
        if (icon.getAttribute('role') !== 'img' || !icon.getAttribute('aria-label') || !icon.title
            || icon.children.length !== 1 || icon.firstElementChild.tagName.toLowerCase() !== 'svg') return false;
        const svg = icon.firstElementChild;
        if (svg.getAttribute('aria-hidden') !== 'true' || getComputedStyle(svg).fill !== 'none') return false;
        colors.add(getComputedStyle(icon).color);
    }
    if (icons.length > 1 && !matchMedia('(forced-colors: active)').matches && colors.size !== icons.length) return false;
}
return true;
