// Observe the actual file/folder history title rows. This does not change DOM,
// styles, focus, pointer state, data or the ancestry layout.
const rows = [...document.querySelectorAll('.zxs-node-disclosure[open] .zxs-version-date')];
if (rows.length === 0) return false;
const close = (a, b) => Number.isFinite(a) && Number.isFinite(b) && Math.abs(a - b) <= .5;
for (const row of rows) {
    const label = row.querySelector(':scope > span');
    if (!label || !label.textContent.trim()) return false;
    const style = getComputedStyle(row), text = getComputedStyle(label);
    const zoom = row.currentCSSZoom, box = row.getBoundingClientRect();
    const height = parseFloat(style.getPropertyValue('--zxs-compact-row-height'));
    if (!(zoom > 0) || !(height > 0) || !close(parseFloat(style.minHeight), height)
        || !close(parseFloat(style.paddingTop), 0) || !close(parseFloat(style.paddingBottom), 0)) return false;
    // A row's text is not another padded/minimum-height row. Its siblings may
    // include a checkbox, button or current-node label and must keep their size.
    for (const edge of ['paddingTop', 'paddingBottom', 'paddingLeft', 'paddingRight']) {
        if (!close(parseFloat(text[edge]), 0)) return false;
    }
    const children = [...row.children].filter(child => child.getBoundingClientRect().height > 0);
    if (children.length === 0) return false;
    const first = children[0], firstStyle = getComputedStyle(first);
    const start = box.left + (parseFloat(style.paddingLeft) + parseFloat(style.borderLeftWidth)) * zoom;
    if (!close(first.getBoundingClientRect().left + parseFloat(firstStyle.paddingLeft) * zoom, start)) return false;
    const content = Math.max(...children.map(child => {
        const css = getComputedStyle(child);
        return child.getBoundingClientRect().height + (parseFloat(css.marginTop) + parseFloat(css.marginBottom)) * zoom;
    }));
    const borders = (parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth)) * zoom;
    if (!close(box.height, Math.max(height * zoom, content + borders))) return false;
}
return true;
