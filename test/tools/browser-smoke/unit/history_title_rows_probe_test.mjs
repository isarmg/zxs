// Tests the browser assertion's positive and negative cases, not CSS rendering.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const source = readFileSync(new URL('../runner/history_title_rows.js', import.meta.url), 'utf8');
const observe = new Function('document', 'getComputedStyle', source);
function fixture({file = false, zoom = 1, wrapped = false} = {}) {
    const css = values => ({paddingTop: '0px', paddingBottom: '0px', paddingLeft: '0px', paddingRight: '0px',
        marginTop: '0px', marginBottom: '0px', borderTopWidth: '0px', borderBottomWidth: '0px', borderLeftWidth: '0px',
        ...values, getPropertyValue: name => name === '--zxs-compact-row-height' ? '36px' : ''});
    const node = (left, height, style = {}) => ({box: {left, height}, style: css(style),
        getBoundingClientRect() {return {left: this.box.left * zoom, height: this.box.height * zoom};}});
    const label = node(file ? 132.2 : 110.8, wrapped ? 43.2 : 21.6);
    label.textContent = file ? 'Date not loaded' : 'Folder snapshot #3';
    const children = file ? [node(110.8, 14.4), label, node(235, 36, {paddingLeft: '10.8px'})] : [label];
    const row = node(100, wrapped ? 43.2 : 36, {paddingLeft: '10.8px', paddingRight: '10.8px', minHeight: '36px'});
    row.children = children;
    row.currentCSSZoom = zoom;
    row.querySelector = selector => {assert.equal(selector, ':scope > span'); return label;};
    const model = {row, label, rows: [row]};
    model.run = () => observe({querySelectorAll: selector => {
        assert.equal(selector, '.zxs-node-disclosure[open] .zxs-version-date'); return model.rows;
    }}, node => node.style);
    return model;
}
for (const file of [false, true]) {
    for (const zoom of [1, 1.25, 2]) {
        for (const wrapped of [false, true]) {
            assert.equal(fixture({file, zoom, wrapped}).run(), true, 'one row, preserved controls and legitimate wrapping');
        }
    }
}
let model = fixture(); model.rows = []; assert.equal(model.run(), false, 'empty fixture is not evidence');
model = fixture(); model.label.textContent = ''; assert.equal(model.run(), false, 'unloaded label');
model = fixture(); model.row.currentCSSZoom = 0; assert.equal(model.run(), false, 'invalid geometry scale');
model = fixture(); model.row.style.minHeight = '44px'; assert.equal(model.run(), false, 'unexpected row minimum');
for (const edge of ['paddingTop', 'paddingBottom', 'paddingLeft', 'paddingRight']) {
    model = fixture(); model.label.style[edge] = '7.2px'; assert.equal(model.run(), false, `doubled label ${edge}`);
}
for (const edge of ['paddingTop', 'paddingBottom']) {
    model = fixture({file: true}); model.row.style[edge] = '7.2px'; assert.equal(model.run(), false, `control wrapper ${edge}`);
}
model = fixture(); model.row.box.height = 50.4; assert.equal(model.run(), false, 'unnecessary taller row');
model = fixture({wrapped: true}); model.row.box.height = 36; assert.equal(model.run(), false, 'wrapped content must not be clipped');
model = fixture(); model.label.box.left += 10.8; assert.equal(model.run(), false, 'first text must have one inset');
model = fixture({file: true}); model.row.children[0].box.left += 10.8; assert.equal(model.run(), false, 'leading checkbox retains row alignment');
model = fixture(); model.label.style.paddingLeft = 'NaN'; assert.equal(model.run(), false, 'invalid layout cannot pass');
console.log('PASS history title-row assertion host cases; not browser rendering evidence');
