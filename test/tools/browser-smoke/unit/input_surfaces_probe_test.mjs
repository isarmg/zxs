// Host-side observer models only. These cases do not establish browser rendering.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';

const source = readFileSync(new URL('../runner/input_surfaces.js', import.meta.url), 'utf8');
const box = (left = 32, width = 240) => ({left, top: 40, right: left + width, bottom: 84, width, height: 44});
const makeControl = (tag = 'INPUT', type = 'text') => {
    const style = {
        opacity: '1', backgroundColor: 'rgba(0, 0, 0, 0)', backgroundImage: 'none', borderImageSource: 'none', boxShadow: 'none',
        outlineWidth: '0px', outlineStyle: 'none', outlineColor: 'rgb(0, 0, 0)',
        '--zxs-contour-extent': '0px', '--zxs-contour-foreground': '0',
        getPropertyValue(property) { return this[property]; },
    };
    for (const side of ['Top', 'Right', 'Bottom', 'Left']) {
        style[`border${side}Width`] = '0px';
        style[`border${side}Style`] = 'none';
        style[`border${side}Color`] = 'rgb(0, 0, 0)';
    }
    return {
        tagName: tag, type, style, box: box(), isConnected: true, visible: true,
        disabled: false, inheritedDisabled: false, focusVisible: false, decorations: [], attrs: {name: 'fixture', 'aria-label': 'Fixture text'},
        get value() { throw new Error('The observer must never read input values'); },
        checkVisibility(options) { assert.equal(options.opacityProperty, false); assert.equal(options.visibilityProperty, true); return this.visible; },
        getBoundingClientRect() { return this.box; },
        getAttribute(name) { return this.attrs[name] ?? null; },
        matches(selector) {
            if (selector === ':disabled') return this.disabled || this.inheritedDisabled;
            assert.equal(selector, ':focus-visible'); return this.focusVisible;
        },
        closest(selector) {
            if (selector === '.zxs-file-search') return this.searchFrame ?? null;
            assert.equal(selector, '.authentication-shell .authentication-card'); return this.anonymous ? {} : null;
        },
        querySelectorAll(selector) { assert.equal(selector, '[data-zxs-contour-decoration]'); return this.decorations; },
    };
};
function fixture() {
    const input = makeControl();
    const textarea = makeControl('TEXTAREA', 'textarea');
    textarea.box = {...box(32, 240), top: 100, bottom: 196, height: 96};
    const f = {input, textarea, controls: [input, textarea], clientWidth: 320, innerWidth: 320};
    f.run = () => runInNewContext(`(function () {${source}\n})()`, {
        innerWidth: f.innerWidth,
        document: {
            documentElement: {clientWidth: f.clientWidth},
            querySelectorAll(selector) { assert.equal(selector, 'input.zxs-input,textarea.zxs-input'); return f.controls; },
        },
        getComputedStyle(node) { return node.style; },
        matchMedia(query) { assert.equal(query, '(forced-colors: active)'); return {matches: f.forced ?? false}; },
    });
    return f;
}
let cases = 0;
function check(label, change, valid, failure) {
    const f = fixture(); change(f);
    const before = f.controls.map(control => ({disabled: control.disabled, inheritedDisabled: control.inheritedDisabled, focusVisible: control.focusVisible, style: {...control.style}, box: {...control.box}}));
    const result = f.run();
    assert.equal(result.valid, valid, label);
    if (failure) assert.ok(result.controls[0].failures.includes(failure), `${label}: ${failure}`);
    assert.deepEqual(f.controls.map(control => ({disabled: control.disabled, inheritedDisabled: control.inheritedDisabled, focusVisible: control.focusVisible, style: {...control.style}, box: {...control.box}})), before, `${label}: observer mutated a control`);
    assert.ok(!JSON.stringify(result).includes('value'), `${label}: report must not contain an input value`);
    cases++;
    return result;
}

check('light text and textarea', () => {}, true);
check('dark border color', f => { for (const node of f.controls) node.style.borderBottomColor = 'rgb(240, 240, 240)'; }, true);
check('transparent keyword', f => { f.input.style.backgroundColor = 'transparent'; }, true);
check('modern transparent color', f => { f.input.style.backgroundColor = 'color(srgb 0 0 0 / 0)'; }, true);
check('disabled text', f => { f.input.disabled = true; f.input.style.opacity = '0.6'; }, true);
check('fieldset-disabled text', f => { f.input.inheritedDisabled = true; f.input.style.opacity = '0.6'; }, true);
check('narrow layout viewport', f => { f.clientWidth = 240; for (const node of f.controls) node.box = box(12, 216); }, true);
const focus = f => { f.input.focusVisible = true; Object.assign(f.input.style, {borderBottomWidth: '3px', borderBottomStyle: 'solid'}); };
check('keyboard focus uses a solid bottom', focus, true);
check('invisible outline color', f => { Object.assign(f.input.style, {outlineWidth: '3px', outlineStyle: 'solid', outlineColor: 'rgba(0, 0, 0, 0)'}); }, true);
check('hidden controls are outside current scene', f => { f.textarea.visible = false; f.textarea.style.borderLeftWidth = '2px'; }, true);
const closed = check('closed disclosure is outside current scene even with layout rects', f => {
    f.input.parentElement = {tagName: 'DETAILS', open: false, children: [{tagName: 'SUMMARY', contains: () => false}]};
    f.input.style.borderTopWidth = '2px';
}, true);
assert.equal(closed.counts.hidden, 1);
check('open disclosure is inspected', f => {
    f.input.parentElement = {tagName: 'DETAILS', open: true, children: []};
    f.input.style.borderTopWidth = '2px';
}, false, 'noRectangle');
check('closed disclosure summary remains visible', f => {
    f.input.parentElement = {tagName: 'DETAILS', open: false, children: [{tagName: 'SUMMARY', contains: () => true}]};
    f.input.style.borderTopWidth = '2px';
}, false, 'noRectangle');
const excluded = check('native controls are excluded', f => {
    for (const type of ['checkbox', 'radio', 'file', 'range', 'color', 'button', 'submit', 'reset', 'image', 'hidden']) {
        const control = makeControl('INPUT', type); control.style.borderTopWidth = '5px'; f.controls.push(control);
    }
}, true);
assert.equal(excluded.counts.excludedTypes, 10);
assert.equal(excluded.controls.length, 2);

for (const side of ['Top', 'Right', 'Left']) {
    check(`${side} rectangular border`, f => { f.input.style[`border${side}Width`] = '2px'; }, false, 'noRectangle');
}
for (const property of ['backgroundImage', 'borderImageSource']) {
    check(`stale G4 ${property}`, f => { f.input.style[property] = 'url("blob:stale-g4")'; }, false, 'noImages');
}
check('rectangular keyboard outline', f => { f.input.focusVisible = true; Object.assign(f.input.style, {borderBottomWidth: '3px', outlineWidth: '2px', outlineStyle: 'solid'}); }, false, 'noOutline');
check('idle dashed bottom is rejected', f => { f.input.style.borderBottomStyle = 'dashed'; }, false, 'bottomStyle');
check('idle solid bottom is rejected', f => { f.input.style.borderBottomStyle = 'solid'; }, false, 'bottomStyle');
check('keyboard dashed bottom is rejected', f => { focus(f); f.input.style.borderBottomStyle = 'dashed'; }, false, 'bottomStyle');
check('transparent focus bottom is not visible', f => { focus(f); f.input.style.borderBottomColor = 'rgba(0, 0, 0, 0)'; }, false, 'bottomStyle');
check('modern transparent focus bottom is not visible', f => { focus(f); f.input.style.borderBottomColor = 'rgb(0 0 0 / 0%)'; }, false, 'bottomStyle');
check('missing focused bottom', f => { focus(f); f.input.style.borderBottomWidth = '0px'; }, false, 'bottomWidth');
check('unfocused thick bottom', f => { f.input.style.borderBottomWidth = '3px'; }, false, 'bottomWidth');
check('focus without thicker bottom', f => { f.input.focusVisible = true; }, false, 'bottomWidth');
check('box shadow creates a second border', f => { f.input.style.boxShadow = 'rgb(0, 0, 0) 0px 0px 0px 2px'; }, false, 'noShadow');
check('opaque native input fill', f => { f.input.style.backgroundColor = 'rgb(255, 255, 255)'; }, false, 'transparentBackground');
check('contour extent must be zero', f => { f.input.style['--zxs-contour-extent'] = '18px'; }, false, 'noContour');
check('contour attribute override must also be zero', f => { f.input.attrs['data-zxs-contour-extent'] = '18px'; }, false, 'noContour');
for (const state of ['paint', 'clip']) {
    check(`stale G4 ${state} state`, f => { f.input.attrs['data-zxs-contour-state'] = state; }, false, 'noContour');
}
check('foreground flag', f => { f.input.style['--zxs-contour-foreground'] = '1'; }, false, 'noForeground');
check('retained G4 child decoration', f => { f.input.decorations.push({}); }, false, 'noForeground');
check('disabled opacity must be distinguishable', f => { f.input.disabled = true; }, false, 'disabledAppearance');
check('fully transparent disabled input', f => { f.input.disabled = true; f.input.style.opacity = '0'; }, false, 'disabledAppearance');
check('zero width', f => { f.input.box = box(32, 0); }, false, 'positiveSize');
check('left viewport overflow', f => { f.input.box = box(-4, 240); }, false, 'withinViewport');
check('right viewport overflow', f => { f.input.box = box(100, 240); }, false, 'withinViewport');
check('classic scrollbar is not available content width', f => { f.clientWidth = 305; f.innerWidth = 320; f.input.box = box(20, 290); }, false, 'withinViewport');
check('detached visible candidate', f => { f.input.isConnected = false; }, false, 'connected');
check('no current text controls cannot silently pass', f => { f.controls = []; }, false);
const anonymous=f=>{f.input.anonymous=true;f.input.style.borderBottomWidth='0px';f.input.style.borderBottomStyle='none';};
check('anonymous idle input is unlined', anonymous, true);
check('anonymous keyboard focus is solid', f=>{anonymous(f);f.input.focusVisible=true;f.input.style.borderBottomWidth='3px';f.input.style.borderBottomStyle='solid';}, true);
check('anonymous policy does not relax other controls', f=>{anonymous(f);f.textarea.style.borderBottomWidth='1px';}, false);
check('anonymous idle dashed line is rejected', f=>{anonymous(f);f.input.style.borderBottomWidth='1px';f.input.style.borderBottomStyle='dashed';}, false, 'bottomWidth');
check('anonymous zero-width dashed style is rejected', f=>{anonymous(f);f.input.style.borderBottomStyle='dashed';}, false, 'bottomStyle');
check('anonymous solid line at rest is rejected', f=>{anonymous(f);f.input.style.borderBottomWidth='1px';f.input.style.borderBottomStyle='solid';}, false, 'bottomWidth');
check('anonymous missing keyboard focus is rejected', f=>{anonymous(f);f.input.focusVisible=true;}, false, 'bottomWidth');
check('anonymous dashed keyboard focus is rejected', f=>{anonymous(f);f.input.focusVisible=true;f.input.style.borderBottomWidth='3px';f.input.style.borderBottomStyle='dashed';}, false, 'bottomStyle');
check('anonymous transparent keyboard focus is rejected', f=>{anonymous(f);f.input.focusVisible=true;f.input.style.borderBottomWidth='3px';f.input.style.borderBottomStyle='solid';f.input.style.borderBottomColor='transparent';}, false, 'bottomStyle');
const folderSearch = f => {
    const input = f.input;
    input.type = 'search';
    input.style.borderBottomWidth = '0px';
    input.style.borderBottomStyle = 'none';
    input.attrs.placeholder = input.attrs['aria-label'];
    const frame = {
        tagName: 'LABEL', textContent: '',
        style: {...input.style, maskImage: 'none', clipPath: 'none', borderImageOutset: '0px'},
        attrs: {'data-zxs-contour-state': 'clip'},
        getAttribute(name) { return this.attrs[name] ?? null; },
        getBoundingClientRect: () => box(),
        querySelectorAll(selector) { assert.equal(selector, 'input'); return [input]; },
    };
    for (const side of ['Top', 'Right', 'Bottom', 'Left']) frame.style[`border${side}Width`] = '0px';
    input.searchFrame = frame;
    input.parentElement = frame;
};
check('folder search fills one label with no underline', folderSearch, true);
const roundedSearchFocus = f => {
    folderSearch(f); f.input.focusVisible = true;
    f.input.searchFrame.attrs['data-zxs-contour-state'] = 'paint';
    Object.assign(f.input.searchFrame.style, {borderImageSource: 'url("blob:g4-focus")', borderImageOutset: '7px'});
};
check('folder search focus uses unmasked G4 paint', roundedSearchFocus, true);
check('folder search must retain native input focus semantics', f => {
    folderSearch(f); f.input.focusVisible = true;
}, false, 'searchSurface');
check('folder search cannot retain a native rectangular outline', f => {
    roundedSearchFocus(f); Object.assign(f.input.searchFrame.style, {outlineWidth: '2px', outlineStyle: 'solid'});
}, false, 'searchSurface');
check('folder search rejects clipped G4 focus corners', f => {
    roundedSearchFocus(f); f.input.searchFrame.style.maskImage = 'url("blob:clip")';
}, false, 'searchSurface');
check('folder search rejects missing G4 focus image', f => {
    roundedSearchFocus(f); f.input.searchFrame.style.borderImageSource = 'none';
}, false, 'searchSurface');
check('folder search rejects an image without focus ring room', f => {
    roundedSearchFocus(f); f.input.searchFrame.style.borderImageOutset = '2px';
}, false, 'searchSurface');
check('folder search cannot keep the old underline', f => {
    folderSearch(f); f.input.style.borderBottomWidth = '1px';
}, false, 'bottomWidth');
check('folder search cannot retain a zero-width dashed line', f => {
    folderSearch(f); f.input.style.borderBottomStyle = 'dashed';
}, false, 'bottomStyle');
check('folder search cannot retain a permanent visible label', f => {
    folderSearch(f); f.input.searchFrame.textContent = 'Search this folder';
}, false, 'searchSurface');
check('folder search requires a stable accessible name', f => {
    folderSearch(f); delete f.input.attrs['aria-label'];
}, false, 'searchSurface');
check('folder search cannot keep an internal left column', f => {
    folderSearch(f); f.input.box = box(112, 160);
}, false, 'searchSurface');
check('folder search cannot leave vertical dead space', f => {
    folderSearch(f); f.input.box = {...f.input.box, top: 44, height: 40};
}, false, 'searchSurface');
check('folder search does not relax unrelated input styles', f => {
    folderSearch(f); f.textarea.style.borderBottomWidth = '1px';
}, false);
check('folder search frame must also be borderless', f => {
    folderSearch(f); f.input.searchFrame.style.borderTopWidth = '1px';
}, false, 'searchSurface');
check('forced colors retain a native input focus outline', f => {
    focus(f); f.forced = true; Object.assign(f.input.style, {outlineWidth: '2px', outlineStyle: 'solid'});
}, true);
check('forced colors cannot lose native input focus', f => {
    focus(f); f.forced = true;
}, false, 'noOutline');
check('forced colors search uses its native outer outline', f => {
    folderSearch(f); f.input.focusVisible = true; f.forced = true;
    Object.assign(f.input.searchFrame.style, {outlineWidth: '2px', outlineStyle: 'solid'});
}, true);
check('forced colors search cannot lose native focus', f => {
    roundedSearchFocus(f); f.forced = true;
}, false, 'searchSurface');
console.log(`PASS ${cases} text input observer model cases; no browser rendering executed`);
