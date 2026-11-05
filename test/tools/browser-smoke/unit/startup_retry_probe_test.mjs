// Observer unit cases only, not production rendering or real keyboard evidence.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const source = readFileSync(new URL('../runner/startup_retry.js', import.meta.url), 'utf8');
const observe = new Function('document', 'getComputedStyle', 'expected', source);
function fixture(expected = 'rest') {
    const css = {display: 'inline-block', visibility: 'visible', opacity: '1', appearance: 'none',
        backgroundColor: 'rgb(239, 239, 239)', boxShadow: 'none',
        outlineStyle: expected === 'keyboard' ? 'solid' : 'none',
        outlineWidth: expected === 'keyboard' ? '2px' : '0px'};
    for (const side of ['Top', 'Right', 'Bottom', 'Left']) css[`border${side}Width`] = '0px';
    for (const corner of ['TopLeft', 'TopRight', 'BottomRight', 'BottomLeft']) css[`border${corner}Radius`] = '8px';
    const node = {box: {width: 150, height: 38}, hovered: expected === 'hover', keyboard: expected === 'keyboard',
        getBoundingClientRect() {return this.box;},
        matches(selector) {
            if (selector === ':hover') return this.hovered;
            assert.equal(selector, ':focus-visible'); return this.keyboard;
        }};
    const document = {activeElement: expected === 'keyboard' ? node : null, node,
        querySelector(selector) {assert.equal(selector, '#zxs-startup-error button'); return this.node;}};
    return {css, node, document, run: () => observe(document, () => css, expected)};
}
for (const mode of ['rest', 'hover', 'keyboard']) assert.equal(fixture(mode).run().valid, true);
for (const side of ['Top', 'Right', 'Bottom', 'Left']) {
    const model = fixture(); model.css[`border${side}Width`] = '2px';
    assert.equal(model.run().valid, false, `ordinary ${side} frame`);
}
for (const corner of ['TopLeft', 'TopRight', 'BottomRight', 'BottomLeft']) {
    const model = fixture('hover'); model.css[`border${corner}Radius`] = '0px';
    assert.equal(model.run().valid, false, `square ${corner} hover`);
}
for (const [key, value] of [['appearance', 'auto'], ['boxShadow', 'rgb(0, 0, 0) 0px 0px 0px 1px'],
    ['visibility', 'hidden'], ['opacity', '0'], ['display', 'none']]) {
    const model = fixture(); model.css[key] = value; assert.equal(model.run().valid, false, key);
}
let model = fixture(); model.document.node = null; assert.equal(model.run().valid, false, 'missing button');
model = fixture(); model.node.box.width = 0; assert.equal(model.run().valid, false, 'empty box');
model = fixture(); model.node.hovered = true; assert.equal(model.run().valid, false, 'rest must be unhovered');
model = fixture('hover'); model.node.hovered = false; assert.equal(model.run().valid, false, 'real hover required');
model = fixture('keyboard'); model.node.keyboard = false; assert.equal(model.run().valid, false, 'keyboard modality required');
model = fixture('keyboard'); model.document.activeElement = null; assert.equal(model.run().valid, false, 'active element required');
for (const [key, value] of [['outlineStyle', 'none'], ['outlineWidth', '0px'], ['outlineWidth', '1px']]) {
    const model = fixture('keyboard'); model.css[key] = value; assert.equal(model.run().valid, false, key);
}
model = fixture(); model.css.outlineStyle = 'solid'; model.css.outlineWidth = '2px';
assert.equal(model.run().valid, false, 'rest has no focus frame');
assert.equal(fixture('unknown').run().valid, false, 'unknown expectation rejected');
console.log('PASS startup retry observer host cases; production browser check remains separate');
