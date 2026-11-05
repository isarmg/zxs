// Host-side observer contracts, not evidence that a real browser painted a ring.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source = readFileSync(new URL('../runner/select_focus.js', import.meta.url), 'utf8');
const css = readFileSync(new URL('../../../../src/web/public/styles/components/select.css', import.meta.url), 'utf8');
assert.match(css, /:focus-visible\s*\{\s*outline: 0;\s*box-shadow: 0 0 0 3px var\(--zxs-bg-page, #fff\), 0 0 0 5px var\(--zxs-focus, Highlight\);\s*--zxs-contour-mode: paint;/);
assert.match(css, /@media \(forced-colors: active\)[\s\S]*outline: 2px solid Highlight;/);
function fixture(expected = 'keyboard') {
    const f = {expected, focused: expected !== 'blurred', focusVisible: expected === 'keyboard' || expected === 'forced', forced: expected === 'forced', ring: [68, 68, 68, 255], gap: [255, 255, 255, 255], border: [255, 255, 255, 255], hover: false, fine: true, row: false, corner: 0, decode: () => {}};
    const keyboard = expected === 'keyboard';
    f.style = {outlineWidth: expected === 'forced' ? '2px' : '0px', outlineStyle: expected === 'forced' ? 'solid' : 'none',
        borderImageSource: keyboard ? 'url("blob:paint")' : 'url("blob:rest")', backgroundImage: 'none',
        boxShadow: 'none', maskImage: 'none', clipPath: 'none', borderImageOutset: keyboard ? '7px' : '2px', borderImageRepeat: 'stretch',
        getPropertyValue(name) {return name === '--zxs-focus' ? '#444' : '#fff';}};
    for (const side of ['Top', 'Right', 'Bottom', 'Left']) f.style[`border${side}Width`] = '0px';
    f.node = {isConnected: true, visible: true, dataset: {zxsContourState: keyboard ? 'paint' : 'clip'}, box: {width: 100, height: 36},
        checkVisibility() {return this.visible;}, getBoundingClientRect() {return this.box;}, matches(selector) {return selector === ':hover' ? f.hover : f.focusVisible;}, closest() {return f.row ? {} : null;}};
    f.run = () => new Promise(resolve => {
        const document = {get activeElement() {return f.focused ? f.node : null;}, querySelector() {return f.node;},
            createElement(tag) {
                assert.equal(tag, 'canvas');
                const canvas = {width: 0, height: 0};
                const context = {fillStyle: '#fff', drawImage() {}, clearRect() {}, fillRect() {},
                    getImageData() {
                        if (canvas.width === 1) return {data: new Uint8ClampedArray(this.fillStyle === '#444' ? [68, 68, 68, 255] : [255, 255, 255, 255])};
                        const data = new Uint8ClampedArray(114 * 50 * 4);
                        const paint = (x, y, rgba) => data.set(rgba, (y * 114 + x) * 4);
                        for (const [distance, rgba] of [[4, f.ring], [2, f.gap], [0, f.border]]) {
                            for (const [x, y] of [[57, 7 - distance], [106 + distance, 25], [57, 42 + distance], [7 - distance, 25]]) paint(x, y, rgba);
                        }
                        for (const [x, y] of [[3, 3], [110, 3], [3, 46], [110, 46]]) paint(x, y, [68, 68, 68, f.corner]);
                        return {data};
                    }};
                canvas.getContext = () => context;
                return canvas;
            }};
        runInNewContext(source, {arguments: ['.zxs-select-trigger', f.expected, resolve], document, getComputedStyle: () => f.style,
            matchMedia: query => ({matches: query === '(forced-colors: active)' ? f.forced : f.fine}), Image: class {naturalWidth = 114; naturalHeight = 50; async decode() {f.decode();}}});
    });
    return f;
}
async function check(name, mutate, valid = false, expected = 'keyboard') {
    const f = fixture(expected); mutate(f);
    const result = await f.run();
    assert.equal(result.valid, valid, `${name}: ${JSON.stringify(result)}`);
}
await check('actual ring and gap around a borderless curved surface', () => {}, true);
await check('square outline still present', f => Object.assign(f.style, {outlineWidth: '2px', outlineStyle: 'solid'}));
await check('focus was removed', f => {f.focused = false;});
await check('keyboard modality missing', f => {f.focusVisible = false;});
await check('hidden control', f => {f.node.visible = false;});
await check('clip cuts off the ring', f => {f.node.dataset.zxsContourState = 'clip';});
await check('mask clips ring', f => {f.style.maskImage = 'url("blob:mask")';});
await check('clip path clips ring', f => {f.style.clipPath = 'path("M0 0H100V36Z")';});
await check('no generated image', f => {f.style.borderImageSource = 'none';});
await check('outset cannot display ring', f => {f.style.borderImageOutset = '0px';});
await check('image dimensions do not match control', f => {f.node.box.width = 90;});
await check('focus color missing from pixels', f => {f.ring = [0, 0, 0, 0];});
await check('gap missing', f => {f.gap = f.ring;});
await check('one-pixel decorative border is rejected', f => {f.style.borderLeftWidth = '1px';});
await check('square corners', f => {f.corner = 255;});
await check('control border grew', f => {f.style.borderTopWidth = '3px';});
await check('stale image', f => {f.decode = () => {f.style.borderImageSource = 'url("blob:other")';};});
await check('disconnected during decoding', f => {f.decode = () => {f.node.isConnected = false;};});
await check('image decode failed', f => {f.decode = () => {throw new Error('decode failed');};});
await check('pointer keeps resting borderless G4 fill', () => {}, true, 'pointer');
await check('pointer must not retain keyboard indicator', f => {f.focusVisible = true;}, false, 'pointer');
await check('pointer must not retain focus ring outset', f => {f.style.borderImageOutset = '7px';}, false, 'pointer');
await check('pointer hover must not add trigger paint', f => {f.hover = true; f.node.dataset.zxsContourState = 'paint';}, false, 'pointer');
await check('pointer hover keeps the resting clipped fill', f => {f.hover = true;}, true, 'pointer');
await check('touch does not require a hover highlight', f => {f.hover = true; f.fine = false;}, true, 'pointer');
await check('nested trigger keeps the resting clipped fill', f => {f.hover = true; f.row = true;}, true, 'pointer');
await check('blurred hovered trigger must not add paint', f => {f.hover = true; f.node.dataset.zxsContourState = 'paint';}, false, 'blurred');
await check('resting fill cannot lose its G4 image', f => {f.style.borderImageSource = 'none';}, false, 'pointer');
await check('blur restores resting paint', () => {}, true, 'blurred');
await check('blur must remove focus ownership', f => {f.focused = true;}, false, 'blurred');
await check('forced colors keeps system outline', () => {}, true, 'forced');
await check('forced colors must not lose indicator', f => {f.style.outlineWidth = '0px';}, false, 'forced');
console.log('PASS 32 Select focus observer model cases; no browser rendering executed');
