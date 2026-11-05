// Observe the real focused Select and its G4 paint. Never restyle or focus it.
const [selector, expected] = arguments;
const done = arguments[arguments.length - 1];
(async () => {
    const node = document.querySelector(selector);
    if (!node) return {valid: false, reason: 'missing Select'};
    const css = getComputedStyle(node), box = node.getBoundingClientRect();
    const focused = document.activeElement === node;
    const focusVisible = node.matches(':focus-visible');
    const outline = parseFloat(css.outlineWidth) > 0 && !['none', 'hidden'].includes(css.outlineStyle);
    const visible = node.checkVisibility({opacityProperty: true, visibilityProperty: true});
    const forced = matchMedia('(forced-colors: active)').matches;
    const borderless = ['Top', 'Right', 'Bottom', 'Left'].every(side => parseFloat(css[`border${side}Width`]) === 0);
    const state = {expected, focused, focusVisible, outline, forced, borderless, contour: node.dataset.zxsContourState};
    if (expected === 'forced') {
        return {...state, valid: visible && borderless && focused && focusVisible && forced && outline && css.outlineStyle === 'solid' && parseFloat(css.outlineWidth) >= 2};
    }
    if (!visible || !borderless || forced || outline || focused !== (expected !== 'blurred') || focusVisible !== (expected === 'keyboard')) {
        return {...state, valid: false, reason: 'wrong focus modality or rectangular outline'};
    }
    if (expected !== 'keyboard') {
        // Pointer hover leaves the trigger's resting fill unchanged. Only the
        // expanded options highlight; the keyboard ring belongs to focus-visible.
        const fill = state.contour === 'clip'
            && css.borderImageSource.startsWith('url("blob:') && parseFloat(css.borderImageOutset) === 2
            && css.borderImageRepeat === 'stretch' && css.backgroundImage === 'none';
        return {...state, valid: fill && css.boxShadow === 'none'};
    }
    const source = css.borderImageSource;
    const match = source.match(/^url\(["']?(blob:[^"')]+)["']?\)$/);
    const outset = parseFloat(css.borderImageOutset);
    if (!match || state.contour !== 'paint' || css.maskImage !== 'none' || css.clipPath !== 'none' || outset < 5 || css.borderImageRepeat !== 'stretch') {
        return {...state, valid: false, reason: 'missing or clipped G4 focus paint'};
    }
    const image = new Image(); image.src = match[1]; await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.naturalWidth; canvas.height = image.naturalHeight;
    const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
    const {width, height} = canvas;
    const pixels = context.getImageData(0, 0, width, height).data;
    const palette = document.createElement('canvas'); palette.width = palette.height = 1;
    const colors = palette.getContext('2d');
    const color = value => {
        colors.clearRect(0, 0, 1, 1); colors.fillStyle = value; colors.fillRect(0, 0, 1, 1);
        return [...colors.getImageData(0, 0, 1, 1).data];
    };
    const focus = color(css.getPropertyValue('--zxs-focus').trim() || 'Highlight');
    const gap = color(css.getPropertyValue('--zxs-bg-page').trim() || '#fff');
    const pixel = (x, y) => [...pixels.slice((Math.floor(y) * width + Math.floor(x)) * 4, (Math.floor(y) * width + Math.floor(x)) * 4 + 4)];
    const matches = (actual, target, alpha = 250) => actual[3] >= alpha && target.slice(0, 3).every((value, i) => Math.abs(actual[i] - value) <= 2);
    const edges = distance => [pixel(width / 2, outset - distance), pixel(width - outset + distance - 1, height / 2), pixel(width / 2, height - outset + distance - 1), pixel(outset - distance, height / 2)];
    const ring = edges(4), separation = edges(2);
    const corners = [[outset - 4, outset - 4], [width - outset + 3, outset - 4], [outset - 4, height - outset + 3], [width - outset + 3, height - outset + 3]].map(([x, y]) => pixel(x, y)[3]);
    const live = getComputedStyle(node);
    const stable = node.isConnected && document.activeElement === node && node.matches(':focus-visible') && live.borderImageSource === source;
    const checks = {
        stable, dimensions: Math.abs(width - box.width - 2 * outset) <= 1 && Math.abs(height - box.height - 2 * outset) <= 1,
        focusPixels: ring.every(p => matches(p, focus)), gapPixels: separation.every(p => matches(p, gap)),
        curvedCorners: corners.every(alpha => alpha === 0),
        unchangedBorder: ['Top', 'Right', 'Bottom', 'Left'].every(side => parseFloat(live[`border${side}Width`]) === 0),
    };
    return {...state, valid: Object.values(checks).every(Boolean), checks, ring, separation, corners};
})().then(done, error => done({valid: false, error: String(error)}));
