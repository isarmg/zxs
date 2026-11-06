// Observer contract models only. No browser, WASM, compositor, or pixels run here.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source = readFileSync(new URL('../runner/select_scroll.js', import.meta.url), 'utf8');
let cases = 0;
function fixture() {
    const listeners = new Map(), callbacks = new Map();
    let nextFrame = 0, timestamp = 0;
    const box = {x: 20, y: 80, width: 200, height: 100};
    const path = {isConnected: true, d: 'M0 0h200v100H0Z M2 2v96h196V2Z',
        css: {fill: 'rgb(31, 37, 42)', fillRule: 'evenodd', stroke: 'none'},
        getAttribute(name) { assert.equal(name, 'd'); return this.d; }};
    const panel = {isConnected: true, open: true, scrollTop: 0, scrollLeft: 0,
        dataset: {zxsContourForegroundAnchor: 'local'}, css: {overflowY: 'visible'},
        getBoundingClientRect() { return {...box}; }, matches(selector) { assert.equal(selector, ':popover-open'); return this.open; },
        querySelector(selector) {
            if (selector === ':scope > .zxs-select-options') return f.missingScroller ? null : scroller;
            assert.equal(selector, ':scope > svg[data-zxs-contour-decoration="foreground"]');
            return f.missingForeground ? null : foreground;
        }};
    const option = {isConnected: true, getBoundingClientRect() {
        return {x: box.x + 1, y: box.y + 1 - (f.frozenContent ? 0 : scroller.scrollTop), width: 198, height: 36};
    }};
    const scroller = {isConnected: true, scrollTop: 0, scrollHeight: 300, clientHeight: 98,
        css: {overflowY: 'auto'}, getBoundingClientRect() { return {x: box.x + 1, y: box.y + 1, width: 198, height: 98}; },
        querySelector(selector) { assert.equal(selector, '[role=option]'); return option; },
        addEventListener(type, callback) { assert.equal(listeners.has(type), false); listeners.set(type, callback); },
        removeEventListener(type, callback) { if (listeners.has(type)) assert.equal(listeners.get(type), callback); listeners.delete(type); },
        contains(hit) { return hit === option; }};
    const foreground = {isConnected: true, visible: true, parentElement: panel,
        css: {position: 'absolute'}, offset: {x: 0, y: 0, width: 0, height: 0},
        checkVisibility() { return this.visible; },
        getBoundingClientRect() { return Object.fromEntries(Object.entries(box).map(([key, value]) => [key, value + this.offset[key]])); },
        querySelector(selector) { assert.equal(selector, 'path'); return path; }};
    const f = {panel, scroller, option, foreground, path, box, listeners, callbacks, missingScroller: false,
        missingForeground: false, frozenContent: false, hit: option, window: {},
        context: {scrollX: 0, scrollY: 0}};
    const requestAnimationFrame = callback => { callbacks.set(++nextFrame, callback); return nextFrame; };
    f.run = operation => new Promise(resolve => runInNewContext(source, {
        ...f.context, arguments: [operation, '.fixture:popover-open', resolve], window: f.window,
        document: {querySelector() { return panel; }, elementFromPoint() { return f.hit; }},
        performance: {now: () => timestamp}, getComputedStyle: node => node.css,
        requestAnimationFrame, cancelAnimationFrame: id => callbacks.delete(id),
    }));
    f.frame = async () => {
        timestamp += 16;
        const current = [...callbacks.values()]; callbacks.clear();
        for (const callback of current) callback(timestamp);
        await Promise.resolve();
    };
    f.scroll = (top, trusted = true) => {
        listeners.get('wheel')?.({isTrusted: trusted});
        scroller.scrollTop = top;
        listeners.get('scroll')?.({target: scroller, isTrusted: trusted});
    };
    f.gesture = async (beforeFrame = () => {}, trusted = true) => {
        await f.frame();
        for (const [index, top] of [40, 100, 60, 20, 80, 0].entries()) {
            f.scroll(top, trusted); beforeFrame(index); await f.frame();
        }
    };
    f.finish = async () => {
        const pending = f.run('finish');
        await f.frame(); await f.frame(); await f.frame();
        const result = await pending;
        assert.equal(listeners.size, 0); assert.equal(callbacks.size, 0);
        assert.equal(f.window.__zxsSelectScrollProbe, undefined);
        return result;
    };
    return f;
}
async function rejectInitial(change, label) {
    const f = fixture(); change(f);
    assert.equal((await f.run('start')).valid, false, label);
    assert.equal(f.listeners.size, 0); assert.equal(f.callbacks.size, 0); cases++;
}
async function rejectDuring(change, label) {
    const f = fixture(); assert.equal((await f.run('start')).valid, true);
    await f.gesture(index => change(f, index));
    assert.equal((await f.finish()).valid, false, label); cases++;
}
let f = fixture();
assert.equal((await f.run('start')).valid, true);
await f.gesture();
let result = await f.finish();
assert.equal(result.valid, true); assert.equal(result.movingFrames, 6);
assert.equal(result.trustedWheels, 6); assert.equal(result.trustedScrolls, 6); cases++;
await rejectInitial(f => { f.foreground.css.position = 'fixed'; }, 'viewport overlay must fail even when stationary geometry aligns');
await rejectInitial(f => { f.panel.dataset.zxsContourForegroundAnchor = 'viewport'; }, 'missing local owner contract');
await rejectInitial(f => { f.panel.css.overflowY = 'auto'; }, 'outer card cannot own scrolling');
await rejectInitial(f => { f.scroller.css.overflowY = 'hidden'; }, 'inner scrolling must remain enabled');
await rejectInitial(f => { f.scroller.scrollHeight = 100; }, 'non-overflowing fixture cannot claim scroll evidence');
await rejectInitial(f => { f.scroller.scrollHeight = f.scroller.clientHeight + 8; }, 'observed Chrome fixture with only 8px overflow must fail');
await rejectInitial(f => { f.scroller.scrollHeight = f.scroller.clientHeight + 23; }, 'overflow below the 24px minimum must fail');
f = fixture(); f.scroller.scrollHeight = f.scroller.clientHeight + 24;
result = await f.run('start');
assert.equal(result.valid, true); assert.equal(result.maximum, 24);
assert.equal((await f.run('cleanup')).valid, true); cases++;
await rejectInitial(f => { f.missingScroller = true; }, 'missing inner scroller');
await rejectInitial(f => { f.missingForeground = true; }, 'missing palette-derived border paint');
await rejectInitial(f => { f.hit = {}; }, 'wheel input must target the actual scroll container');
await rejectInitial(f => { f.panel.open = false; }, 'closed panel');
for (const axis of ['x', 'y', 'width', 'height']) {
    await rejectDuring((f, index) => { f.foreground.offset[axis] = index === 2 ? 2 : 0; }, `one bad moving frame ${axis} cannot be hidden by final alignment`);
}
await rejectDuring((f, index) => { f.box.y = index === 2 ? 82 : 80; }, 'border and box bouncing together must fail');
await rejectDuring((f, index) => { f.foreground.css.position = index === 2 ? 'fixed' : 'absolute'; }, 'one fixed-overlay frame');
await rejectDuring((f, index) => { f.foreground.visible = index !== 2; }, 'border disappearing during the gesture');
await rejectDuring((f, index) => { f.panel.open = index !== 2; }, 'inner scrolling must not close the menu');
await rejectDuring((f, index) => { f.path.css.fill = index === 2 ? 'rgb(90, 90, 90)' : 'rgb(31, 37, 42)'; }, 'transient wrong-palette border');
await rejectDuring((f, index) => { f.path.d = index === 2 ? 'M0 0' : 'M0 0h200v100H0Z M2 2v96h196V2Z'; }, 'path mutation during scroll');
await rejectDuring((f, index) => { f.panel.scrollTop = index === 2 ? 10 : 0; }, 'scrolling the painted outer box');
await rejectDuring((f, index) => { f.foreground.isConnected = index !== 2; }, 'detached paint during scroll');
await rejectDuring(f => { f.frozenContent = true; }, 'scrollTop without actual option movement is insufficient');
f = fixture(); assert.equal((await f.run('start')).valid, true);
await f.gesture(() => {}, false); assert.equal((await f.finish()).valid, false, 'synthetic scroll/wheel events'); cases++;
f = fixture(); assert.equal((await f.run('start')).valid, true);
await f.frame(); for (const top of [40, 100, 60, 20, 80, 0]) f.scroll(top);
assert.equal((await f.finish()).valid, false, 'only stationary RAFs after a burst cannot pass'); cases++;
f = fixture(); assert.equal((await f.run('start')).valid, true);
await f.frame(); for (const top of [20, 40, 60, 80, 100]) { f.scroll(top); await f.frame(); }
assert.equal((await f.finish()).valid, false, 'one direction cannot claim a reversing gesture'); cases++;
f = fixture(); assert.equal((await f.run('start')).valid, true);
await f.gesture(); for (let i = 0; i < 256; i++) await f.frame();
assert.equal((await f.finish()).valid, false, 'truncated frame collection cannot pass'); cases++;
f = fixture(); assert.equal((await f.run('start')).valid, true);
assert.equal((await f.run('start')).valid, false, 'duplicate install must not leak the first observer');
assert.equal((await f.run('cleanup')).valid, true);
assert.equal(f.listeners.size, 0); assert.equal(f.callbacks.size, 0);
assert.equal((await f.run('cleanup')).valid, true);
assert.equal((await f.run('finish')).valid, false); cases++;
console.log(`PASS ${cases} Select fast-scroll observer model cases; no browser rendering executed`);
