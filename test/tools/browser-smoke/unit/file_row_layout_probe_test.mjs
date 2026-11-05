// Host model of the observation contract, not CSS rendering or browser evidence.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const source = readFileSync(new URL('../runner/file_row_layout.js', import.meta.url), 'utf8');
const observe = new Function('document', 'getComputedStyle', 'matchMedia', source);
const rect = (width, top, height = 36) => ({left: 0, top, width, height, bottom: top + height});
function fixture({count = 3, width = 960, viewport = width, extent = 18, pagination = true, zoom = 1} = {}) {
    const screen = box => Object.fromEntries(Object.entries(box).map(([key, value]) => [key, value * zoom]));
    const style = values => ({getPropertyValue(name) {
        return {'--zxs-contour-extent': `${extent}px`, '--zxs-block-border': '#dedede'}[name] ?? '';
    }, ...values});
    const separator = (present, width) => style({content: present ? '""' : 'none',
        left: `${extent}px`, right: `${extent}px`, width: `${width - extent * 2}px`,
        borderBottomWidth: '1px', borderBottomStyle: 'solid', borderBottomColor: '#dedede', pointerEvents: 'none'});
    const row = (index, header = false) => {
        const node = {selected: false, hovered: false, box: rect(width, index * 36),
            style: style({display: 'table-row', backgroundColor: 'rgba(0, 0, 0, 0)', clipPath: 'none', maskImage: 'none'}),
            line: separator(false, viewport),
            cells: [{style: style({borderBottomWidth: '0px'})}],
            getBoundingClientRect() { return screen(this.box); },
            getAttribute(name) { assert.equal(name, 'aria-selected'); return String(this.selected); },
            matches(selector) { assert.equal(selector, ':hover'); return this.hovered; },
            querySelector(selector) {
                if (selector === ':scope > :is(th,td) > .zxs-file-row-decoration') return this.decoration;
                assert.equal(selector, ':scope > td > .zxs-file-row-decoration > .zxs-file-row-surface');
                return this.surface;
            }};
        node.decoration = {getBoundingClientRect: () => screen(node.box), line: node.line,
            style: style({overflowX: 'clip', overflowY: 'clip'}),
            contains: child => child === node.surface, querySelectorAll: () => []};
        if (!header) node.surface = {tabIndex: -1, state: 'paint', hidden: 'true', box: {...node.box, width: viewport},
            style: style({borderImageSource: 'url("blob:real-output")', borderImageSlice: '49% fill', pointerEvents: 'none', opacity: '0'}),
            getBoundingClientRect() { return screen(this.box); },
            getAttribute(name) { return {'aria-hidden': this.hidden, 'data-zxs-contour-state': this.state}[name]; }};
        return node;
    };
    const rows = Array.from({length: count}, (_, index) => row(index + 1));
    const header = row(0, true);
    const scroll = {scrollLeft: 0, clientLeft: 0, clientWidth: Math.round(viewport), offsetWidth: Math.round(viewport), currentCSSZoom: zoom,
        getBoundingClientRect: () => screen(rect(viewport, 0, (count + 1) * 36))};
    const table = {parentElement: scroll, tBodies: [{rows}], tHead: {rows: [header]}};
    const nav = pagination ? {line: separator(false, viewport), getBoundingClientRect: () => screen(rect(viewport, (count + 1) * 36))} : null;
    const panel = {style: style({}), querySelector(selector) {return {'.zxs-file-table': table, '.zxs-file-pagination': nav}[selector];}};
    const document = {documentElement: {scrollWidth: viewport, clientWidth: viewport},
        querySelector(selector) { assert.equal(selector, '.zxs-file-list-panel'); return panel; },
        createElement(tag) {
            assert.equal(tag, 'canvas');
            const context = {fillStyle: '', fillRect() {}, getImageData() {
                return {data: [...this.fillStyle.slice(1).match(/../g).map(value => parseInt(value, 16)), 255]};
            }};
            return {getContext: () => context};
        }};
    const state = {rows, header, panel, nav, scroll, document, pointer: true};
    state.setScroll = offset => {
        scroll.scrollLeft = offset;
        for (const node of [header, ...rows]) {
            node.box.left = -offset; node.line.left = `${offset + extent}px`;
        }
    };
    state.run = () => observe(document, (node, pseudo) => pseudo ? node.line : node.style, () => ({matches: state.pointer}));
    return state;
}
let checks = 0;
function valid(model) { const result = model.run(); assert.equal(result.valid, true, JSON.stringify(result)); checks++; }
function invalid(change) { const model = fixture(); change(model); assert.equal(model.run().valid, false); checks++; }
for (const count of [0, 1, 3, 5]) {
    for (const viewport of [288, 358, 948, 1408]) {
        for (const pagination of [false, true]) {
            const width = Math.max(viewport, 600), model = fixture({count, viewport, width, pagination});
            const offsets = width > viewport ? [0, .5, width - viewport, 0] : [0];
            for (const offset of offsets) { model.setScroll(offset); valid(model); }
        }
    }
}
// Real CSS zoom scales bounding rects, not computed CSS lengths. Keep the
// screen-space 0.5px tolerance and reject a displaced half-pixel highlight.
for (const zoom of [1.25, 2]) {
    const model = fixture({count:5, width:600, viewport:135.5, zoom});
    for (const offset of [0.5, 1.5, 7.5, 600 - 135.5]) { model.setScroll(offset); valid(model); }
}
const displaced = fixture({width:600, viewport:135.5, zoom:2});
displaced.setScroll(.5); displaced.rows[0].surface.box.left = -.5;
assert.equal(displaced.run().valid, false); checks++;
invalid(model => {model.scroll.currentCSSZoom = 0;});
invalid(model => {model.scroll.clientLeft = 1;});
invalid(model => {model.scroll.offsetWidth += 15;});
valid(fixture({count: 3, extent: 20}));
for (const index of [0, 1, 2]) {
    let model = fixture(); model.rows[index].hovered = true; model.rows[index].surface.style.opacity = '1'; valid(model);
    model = fixture(); model.rows[index].selected = true; model.rows[index].surface.style.opacity = '1'; valid(model);
    model = fixture(); model.pointer = false; model.rows[index].hovered = true; valid(model);
}
for (const node of ['header', 'nav']) {
    invalid(model => {model[node].line.content = '""';});
}
for (const index of [0, 1, 2]) {
    invalid(model => {model.rows[index].line.content = '""';});
}
invalid(model => {model.rows[1].surface = null;});
invalid(model => {model.rows[1].surface.box.width = 200;});
invalid(model => {model.rows[1].surface.state = 'fallback';});
invalid(model => {model.rows[1].surface.style.borderImageSource = 'none';});
invalid(model => {model.rows[1].surface.style.borderImageSlice = '49%';});
invalid(model => {model.rows[1].surface.style.pointerEvents = 'auto';});
invalid(model => {model.rows[1].surface.tabIndex = 0;});
invalid(model => {model.rows[1].surface.hidden = 'false';});
invalid(model => {model.rows[1].surface.style.opacity = '1';});
invalid(model => {model.rows[1].selected = true;});
invalid(model => {model.rows[1].hovered = true;});
invalid(model => {model.rows[1].style.backgroundColor = '#ffffff';});
invalid(model => {model.rows[1].style.display = 'block';});
invalid(model => {model.rows[1].decoration.style.overflowX = 'visible';});
invalid(model => {model.rows[1].decoration.querySelectorAll = () => [{}];});
invalid(model => {model.rows[1].style.clipPath = 'inset(0 round 18px)';});
invalid(model => {model.rows[1].style.maskImage = 'url("blob:clips-focus")';});
invalid(model => {model.rows[1].cells[0].style.borderBottomWidth = '1px';});
invalid(model => {model.header.cells[0].style.borderBottomWidth = '1px';});
invalid(model => {model.document.documentElement.scrollWidth += 20;});
for (const offset of [0, 312]) {
    const wide = fixture({width: 600, viewport: 288}); wide.setScroll(offset);
    wide.rows[1].surface.box = {...wide.rows[1].box};
    assert.equal(wide.run().valid, false); checks++;
    const line = fixture({width: 600, viewport: 288}); line.setScroll(offset);
    line.rows[1].line.content = '""';
    assert.equal(line.run().valid, false); checks++;
}
const empty = fixture({count: 0}); empty.header.line.content = '""';
assert.equal(empty.run().valid, false); checks++;
console.log(`PASS ${checks} file-row observation model cases (not browser rendering)`);
