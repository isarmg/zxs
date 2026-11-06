import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('../runner/node_cards.js', import.meta.url), 'utf8');

function fixture() {
    const svg = {tagName: 'svg', getAttribute: name => name === 'aria-hidden' ? 'true' : null};
    const icon = {title: 'Renamed', children: [svg], firstElementChild: svg,
        getAttribute: name => ({role: 'img', 'aria-label': 'Renamed'}[name] ?? null)};
    const title = {textContent: 'Long original note', matches: () => false};
    const row = {textContent: '', querySelectorAll: () => [icon],
        matches: () => false,
        getBoundingClientRect: () => ({top: 36, height: 36})};
    const attributes = {'aria-label': 'Long original note; Renamed; Show details',
        'aria-current': 'true', 'data-zxs-contour-state': 'paint'};
    const card = {tagName: 'SUMMARY', parentElement: {tagName: 'DETAILS'},
        title: title.textContent, currentCSSZoom: 1, children: [title, row],
        querySelector: selector => selector.includes('title') ? title : row,
        getAttribute: name => attributes[name] ?? null,
        getBoundingClientRect: () => ({top: 0, width: 80, height: 72})};
    const styles = new Map([
        [card, {fontSize: '16px', fontFamily: 'monospace', getPropertyValue: () => '36px',
            borderImageSource: 'url("blob:g4-card")', borderImageSlice: '4 fill',
            borderImageOutset: '4px', maskImage: 'none'}],
        [title, {textOverflow: 'ellipsis', whiteSpace: 'nowrap'}],
        [icon, {color: 'rgb(0,0,200)'}], [svg, {fill: 'none'}],
    ]);
    const document = {fonts: {status: 'loaded'}, querySelectorAll: () => [card],
        createElement: () => ({getContext: () => ({measureText: () => ({width: 80})})})};
    const context = {document, getComputedStyle: element => styles.get(element),
        matchMedia: () => ({matches: false})};
    return {card, title, row, icon, svg, styles, document, attributes,
        run: () => vm.runInNewContext(`(()=>{${source}})()`, context)};
}

test('two real rows, 10ch, icon label and full original title satisfy the probe', () => {
    assert.equal(fixture().run(), true);
});
test('a visible action word fails the icon-only requirement', () => {
    const f = fixture(); f.row.textContent = 'Renamed'; assert.equal(f.run(), false);
});
test('a changed card width or third row fails', () => {
    const f = fixture(); f.card.getBoundingClientRect = () => ({top: 0, width: 96, height: 72});
    assert.equal(f.run(), false);
    const g = fixture(); g.card.children.push({matches: () => false}); assert.equal(g.run(), false);
});
test('missing accessible icon name fails even with an SVG', () => {
    const f = fixture(); f.icon.getAttribute = name => name === 'role' ? 'img' : null;
    assert.equal(f.run(), false);
});
test('silently truncated stored title fails', () => {
    const f = fixture(); f.card.title = 'Long...'; assert.equal(f.run(), false);
});
test('no cards or fonts still loading cannot pass', () => {
    const f = fixture(); f.document.querySelectorAll = () => []; assert.equal(f.run(), false);
    const g = fixture(); g.document.fonts.status = 'loading'; assert.equal(g.run(), false);
});
test('filled icons and a non-native disclosure cannot pass', () => {
    const f = fixture(); f.styles.get(f.svg).fill = 'red'; assert.equal(f.run(), false);
    const g = fixture(); g.card.tagName = 'DIV'; assert.equal(g.run(), false);
});
test('the old current-card inset fallback and an unpainted card fail', () => {
    for (const state of ['fallback', undefined]) {
        const f = fixture(); f.attributes['data-zxs-contour-state'] = state;
        f.attributes['data-zxs-contour-reason'] = 'inset-or-unsupported-shadow';
        f.styles.get(f.card).borderImageSource = 'none';
        f.styles.get(f.card).maskImage = 'none';
        assert.equal(f.run(), false);
    }
});
test('current paint keeps the ring outside the original clip while historical cards still clip', () => {
    const f = fixture(); f.attributes['data-zxs-contour-state'] = 'clip';
    f.styles.get(f.card).maskImage = 'url("blob:g4-mask")';
    assert.equal(f.run(), false);
    const current = fixture(), historical = fixture();
    historical.attributes['aria-current'] = 'false';
    historical.attributes['data-zxs-contour-state'] = 'clip';
    historical.styles.get(historical.card).maskImage = 'url("blob:g4-mask")';
    historical.styles.get(historical.card).borderImageOutset = '2px';
    for (const [node, style] of historical.styles) current.styles.set(node, style);
    current.document.querySelectorAll = () => [current.card, historical.card];
    assert.equal(current.run(), true);
    historical.styles.get(historical.card).maskImage = 'none';
    assert.equal(current.run(), false);
    historical.attributes['data-zxs-contour-state'] = 'paint';
    assert.equal(current.run(), false);
});
test('a mode marker without actual G4 output or an unclipped current ring fails', () => {
    for (const [property, value] of [['borderImageSource', 'none'], ['borderImageSlice', '4'],
        ['maskImage', 'url("blob:clips-the-ring")'], ['borderImageOutset', '2px'], ['borderImageOutset', '']]) {
        const f = fixture(); f.styles.get(f.card)[property] = value; assert.equal(f.run(), false);
    }
});
test('non-current cards are checked too and the fixture must include a current card', () => {
    const f = fixture(), historical = fixture(); historical.attributes['aria-current'] = 'false';
    historical.attributes['data-zxs-contour-state'] = 'fallback';
    for (const [node, style] of historical.styles) f.styles.set(node, style);
    f.document.querySelectorAll = () => [f.card, historical.card];
    assert.equal(f.run(), false);
    const g = fixture(); g.attributes['aria-current'] = 'false'; assert.equal(g.run(), false);
});
test('the same card must clear and restore the old mask when current identity changes', () => {
    const f = fixture(), otherCurrent = fixture();
    for (const [node, style] of otherCurrent.styles) f.styles.set(node, style);
    f.document.querySelectorAll = () => [f.card, otherCurrent.card];
    assert.equal(f.run(), true);
    f.attributes['aria-current'] = 'false';
    assert.equal(f.run(), false);
    f.attributes['data-zxs-contour-state'] = 'clip';
    f.styles.get(f.card).maskImage = 'url("blob:historical-mask")';
    assert.equal(f.run(), true);
    f.attributes['aria-current'] = 'true';
    assert.equal(f.run(), false);
    f.attributes['data-zxs-contour-state'] = 'paint';
    f.styles.get(f.card).maskImage = 'none';
    assert.equal(f.run(), true);
});
test('G4-owned foreground decoration is not a third semantic card row', () => {
    const f = fixture(); f.card.children.push({matches: selector => selector === 'svg[data-zxs-contour-decoration="foreground"]'});
    assert.equal(f.run(), true);
});
test('long Chinese titles and explicit empty-note captions retain the 10ch geometry', () => {
    for (const caption of ['这是一个超过卡面宽度的中文备注标题 😀', '无备注 #42']) {
        const f = fixture(); f.card.title = caption; f.title.textContent = caption;
        f.attributes['aria-label'] = `${caption}; Renamed; Show details`;
        assert.equal(f.run(), true);
    }
});
