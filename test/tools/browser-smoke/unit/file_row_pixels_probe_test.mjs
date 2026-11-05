// Pixel-observer model only. CI's browser-smoke separately captures real Chrome PNGs.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source = readFileSync(new URL('../runner/file_row_pixels.js', import.meta.url), 'utf8');
const panel = [242, 242, 242, 255], fill = [221, 221, 221, 255], focus = [68, 68, 68, 255];
function fixture(kind = 'row') {
    const width = 64, height = 36, pixels = new Uint8ClampedArray(width * height * 4);
    const put = (x, y, value) => pixels.set(value, (y * width + x) * 4);
    for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
        put(x, y, kind === 'row' && x >= 18 && x < width - 18 ? fill : panel);
    }
    if (kind === 'focus') for (let y = 0; y < height; y++) for (const x of [0, 1, width - 2, width - 1]) put(x, y, focus);
    const scene = {kind, panel: '#f2f2f2', fill: '#dddddd', color: '#444444', ring: 2};
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
// The shared hover token, not the former page-white fill, is the oracle.
model = fixture(); model.scene.fill = '#ffffff'; await check(model, false);
model = fixture(); model.scene.panel = '#000000'; await check(model, false);
model = fixture(); model.scene.fill = model.scene.panel; await check(model, false);
await check(fixture('focus'), true);
for (const xs of [[0, 1], [62, 63]]) {
    const clipped = fixture('focus'); for (const x of xs) for (let y = 0; y < 36; y++) clipped.put(x, y, panel);
    await check(clipped, false);
}
model = fixture('focus'); model.scene.color = '#000000'; await check(model, false);
function searchFocus(focused = true) {
    const model = fixture('search-focus');
    Object.assign(model.scene, {focused, color: '#444444', page: '#f2f2f2', extent: 9,
        clip: {width: 64, height: 36}, frame: {x: 8, y: 8, width: 48, height: 20}});
    if (focused) {
        for (const [x, y] of [[32, 4], [59, 18], [32, 31], [4, 18]]) model.put(x, y, focus);
        for (const [cx, cy] of [[10, 10], [53, 10], [53, 25], [10, 25]]) {
            for (let y = cy - 1; y <= cy + 1; y++) for (let x = cx - 1; x <= cx + 1; x++) model.put(x, y, focus);
        }
    }
    return model;
}
await check(searchFocus(), true);
await check(searchFocus(false), true);
for (const [x, y] of [[4, 4], [59, 4], [59, 31], [4, 31]]) {
    const square = searchFocus(); square.put(x, y, focus); await check(square, false);
}
for (const [x, y] of [[32, 4], [59, 18], [32, 31], [4, 18]]) {
    const missing = searchFocus(); missing.put(x, y, panel); await check(missing, false);
    const retained = searchFocus(false); retained.put(x, y, focus); await check(retained, false);
}
for (const [x, y] of [[32, 6], [57, 18], [32, 29], [6, 18]]) {
    const noGap = searchFocus(); noGap.put(x, y, focus); await check(noGap, false);
}
for (const [x, y] of [[10, 10], [53, 10], [53, 25], [10, 25]]) {
    const clipped = searchFocus();
    for (let cy = y - 1; cy <= y + 1; cy++) for (let cx = x - 1; cx <= x + 1; cx++) clipped.put(cx, cy, panel);
    await check(clipped, false);
}
// A real light-theme PNG at 99b9628d had 96/131 on the diagonal but
// exact 68-valued focus pixels beside it. Only the sampling footprint changes;
// accepting antialias-only pixels would weaken the original color threshold.
const curveCenters = [[10, 10], [53, 10], [53, 25], [10, 25]];
const eraseCurve = (model, index) => {
    const [cx, cy] = curveCenters[index];
    for (let y = cy - 1; y <= cy + 1; y++) for (let x = cx - 1; x <= cx + 1; x++) model.put(x, y, panel);
};
model = searchFocus();
for (const [cx, cy] of curveCenters) {
    for (let y = cy - 1; y <= cy + 1; y++) for (let x = cx - 1; x <= cx + 1; x++) {
        const diagonal = (cx === 10) === (cy === 10) ? x - cx === y - cy : x - cx === cy - y;
        if (diagonal) model.put(x, y, [96, 96, 96, 255]);
    }
}
await check(model, true);
// A full rectangular ring has abundant focus-colored pixels in every corner,
// but must still fail the separate G4 outer-corner background requirement.
const rectangular = searchFocus(false); rectangular.scene.focused = true;
for (let y = 4; y <= 31; y++) for (let x = 4; x <= 59; x++) {
    if (x <= 5 || x >= 58 || y <= 5 || y >= 30) rectangular.put(x, y, focus);
}
await check(rectangular, false);
// Fractional border-box positions must not duplicate a screenshot pixel or
// assume matching left/right raster phases.
model = searchFocus();
Object.assign(model.scene.frame, {x: 8.3, y: 8.25, width: 47.7, height: 19.75});
await check(model, true);
for (let corner = 0; corner < 4; corner++) {
    const [x, y] = curveCenters[corner];
    const one = searchFocus(); eraseCurve(one, corner); one.put(x, y, focus);
    await check(one, false);
    const straight = searchFocus(); eraseCurve(straight, corner);
    straight.put(x, y, focus); straight.put(x + 1, y, focus);
    await check(straight, false);
    const antialiasOnly = searchFocus(); eraseCurve(antialiasOnly, corner);
    antialiasOnly.put(x, y, [96, 96, 96, 255]); antialiasOnly.put(x + 1, y + 1, [96, 96, 96, 255]);
    await check(antialiasOnly, false);
    // Four clear midpoint samples cannot hide an isolated stale corner ring.
    const staleCorner = searchFocus(false); staleCorner.put(x, y, focus);
    await check(staleCorner, false);
}
// Focus-colored text/caret pixels beyond the corner reach are not a stale ring.
model = searchFocus(false); model.put(20, 20, focus); await check(model, true);
// The pixel observer must work with independent dark focus and page colors.
const darkFocus = [95, 168, 200, 255], darkPage = [11, 20, 26, 255];
for (const focused of [false, true]) {
    const dark = fixture('search-focus');
    for (let y = 0; y < 36; y++) for (let x = 0; x < 64; x++) dark.put(x, y, darkPage);
    Object.assign(dark.scene, {focused, color: '#5fa8c8', page: '#0b141a', extent: 9,
        clip: {width: 64, height: 36}, frame: {x: 8, y: 8, width: 48, height: 20}});
    if (focused) {
        for (const [x, y] of [[32, 4], [59, 18], [32, 31], [4, 18]]) dark.put(x, y, darkFocus);
        for (const [cx, cy] of curveCenters) {
            dark.put(cx, cy, darkFocus); dark.put(cx + 1, cy + 1, darkFocus);
        }
    }
    await check(dark, true);
}
model = fixture('unknown'); await check(model, false);
console.log(`PASS ${checks} screenshot pixel-observer model cases (not browser screenshots)`);
