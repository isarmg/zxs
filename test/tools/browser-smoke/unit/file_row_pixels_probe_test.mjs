// Pixel-observer model only. CI's browser-smoke separately captures real Chrome PNGs.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source = readFileSync(new URL('../runner/file_row_pixels.js', import.meta.url), 'utf8');
const panel = [242, 242, 242, 255], fill = [255, 255, 255, 255], focus = [68, 68, 68, 255];
function fixture(kind = 'row') {
    const width = 64, height = 36, pixels = new Uint8ClampedArray(width * height * 4);
    const put = (x, y, value) => pixels.set(value, (y * width + x) * 4);
    for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
        put(x, y, kind === 'row' && x >= 18 && x < width - 18 ? fill : panel);
    }
    if (kind === 'focus') for (let y = 0; y < height; y++) for (const x of [0, 1, width - 2, width - 1]) put(x, y, focus);
    const scene = {kind, panel: '#f2f2f2', fill: '#ffffff', color: '#444444', ring: 2};
    let revoked = false;
    class Image { naturalWidth = width; naturalHeight = height; async decode() {} }
    class Blob {}
    const document = {createElement(tag) {
        assert.equal(tag, 'canvas'); let drawn = false;
        const context = {fillStyle: '', drawImage() {drawn = true;}, fillRect() {},
            getImageData() {return {data: drawn ? pixels : [...this.fillStyle.slice(1).match(/../g).map(value => parseInt(value, 16)), 255]};}};
        return {width: 0, height: 0, getContext: () => context};
    }};
    return {put, scene, async run() {
        const result = await new Promise(resolve => runInNewContext(source, {
            arguments: ['AAAA', scene, resolve], Image, Blob, document,
            atob: () => '\0\0\0', Uint8Array,
            URL: {createObjectURL: () => 'blob:screenshot', revokeObjectURL: () => {revoked = true;}},
        }));
        assert.equal(revoked, true); return result;
    }};
}
let checks = 0;
async function check(model, expected) { const result = await model.run(); assert.equal(result.valid, expected, JSON.stringify(result)); checks++; }
await check(fixture(), true);
for (const [x, y] of [[2, 2], [61, 2], [61, 33], [2, 33]]) {
    const clipped = fixture(); clipped.put(x, y, fill); await check(clipped, false);
}
for (const y of [2, 33]) {
    const hidden = fixture(); hidden.put(32, y, panel); await check(hidden, false);
}
let model = fixture(); model.scene.fill = '#000000'; await check(model, false);
model = fixture(); model.scene.panel = '#000000'; await check(model, false);
model = fixture(); model.scene.fill = model.scene.panel; await check(model, false);
await check(fixture('focus'), true);
for (const xs of [[0, 1], [62, 63]]) {
    const clipped = fixture('focus'); for (const x of xs) for (let y = 0; y < 36; y++) clipped.put(x, y, panel);
    await check(clipped, false);
}
model = fixture('focus'); model.scene.color = '#000000'; await check(model, false);
model = fixture('unknown'); await check(model, false);
console.log(`PASS ${checks} screenshot pixel-observer model cases (not browser screenshots)`);
