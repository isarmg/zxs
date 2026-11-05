// Observe a real open Select during native WebDriver wheel input. This probe
// never changes product DOM, styles, scroll offsets, focus, or event delivery.
// Pixel decoding belongs to select_borders.js before and after this gesture.
const [operation, selector] = arguments;
const done = arguments[arguments.length - 1];
(async () => {
    const key = '__zxsSelectScrollProbe';
    if (operation === 'cleanup') {
        window[key]?.stop();
        delete window[key];
        return {valid: true};
    }
    if (operation === 'finish') {
        const probe = window[key];
        if (!probe) throw new Error('Select scroll observer was not started');
        // Drain pending native scroll delivery, but exclude these quiet frames
        // from the in-motion evidence gathered while WebDriver was scrolling.
        probe.scrolling = false;
        await new Promise(requestAnimationFrame);
        await new Promise(requestAnimationFrame);
        probe.stop();
        delete window[key];
        return probe.result();
    }
    if (operation !== 'start') throw new Error(`Unknown Select scroll operation: ${operation}`);
    if (window[key]) throw new Error('Select scroll observer is already active');
    const panel = document.querySelector(selector);
    if (!panel || !panel.matches(':popover-open')) throw new Error('An open Select panel is required');
    const scroller = panel.querySelector(':scope > .zxs-select-options');
    const option = scroller?.querySelector('[role=option]');
    const foreground = panel.querySelector(':scope > svg[data-zxs-contour-decoration="foreground"]');
    const path = foreground?.querySelector('path');
    if (!scroller || !option || !foreground || !path) throw new Error('Missing Select scroller or local foreground');
    const maximum = scroller.scrollHeight - scroller.clientHeight;
    if (maximum < 24) throw new Error(`Select needs natural overflow of at least 24px, found ${maximum}`);
    const box = node => {
        const {x, y, width, height} = node.getBoundingClientRect();
        return {x, y, width, height};
    };
    const aligned = (actual, expected) => Object.keys(expected).every(name =>
        Number.isFinite(actual[name]) && Math.abs(actual[name] - expected[name]) <= .5);
    const initial = {panel: box(panel), top: scroller.scrollTop, optionY: box(option).y,
        pageX: scrollX, pageY: scrollY, path: path.getAttribute('d'), fill: getComputedStyle(path).fill};
    const samples = [];
    let raf, trustedWheels = 0, trustedScrolls = 0, overflow = false;
    const probe = {scrolling: true};
    const sample = (kind, timestamp) => {
        if (samples.length === 256) { overflow = true; return; }
        try {
            const panelBox = box(panel), foregroundBox = box(foreground);
            const css = getComputedStyle(foreground), paint = getComputedStyle(path);
            const panelCss = getComputedStyle(panel), scrollCss = getComputedStyle(scroller);
            const currentTop = scroller.scrollTop, optionY = box(option).y;
            const checks = {
                connected: [panel, scroller, option, foreground, path].every(node => node.isConnected),
                visible: panel.matches(':popover-open') && foreground.checkVisibility({opacityProperty: true, visibilityProperty: true}),
                local: css.position === 'absolute' && foreground.parentElement === panel && panel.dataset.zxsContourForegroundAnchor === 'local',
                aligned: aligned(foregroundBox, panelBox), stableBox: aligned(panelBox, initial.panel),
                stablePaint: path.getAttribute('d') === initial.path && paint.fill === initial.fill && paint.fillRule === 'evenodd' && paint.stroke === 'none',
                innerScroll: panelCss.overflowY === 'visible' && scrollCss.overflowY === 'auto' && panel.scrollTop === 0 && panel.scrollLeft === 0 && scrollX === initial.pageX && scrollY === initial.pageY,
                movedContent: Math.abs(optionY - initial.optionY + currentTop - initial.top) <= .5,
            };
            samples.push({kind, timestamp, scrolling: probe.scrolling, top: currentTop,
                panel: panelBox, foreground: foregroundBox, optionY,
                valid: Object.values(checks).every(Boolean), checks});
        } catch (error) {
            samples.push({kind, timestamp, scrolling: probe.scrolling, valid: false, error: String(error)});
        }
    };
    const onWheel = event => { if (event.isTrusted) trustedWheels++; };
    const onScroll = event => {
        if (event.target !== scroller) return;
        if (event.isTrusted) trustedScrolls++;
        sample('scroll', performance.now());
    };
    const onFrame = timestamp => {
        sample('frame', timestamp);
        raf = requestAnimationFrame(onFrame);
    };
    probe.stop = () => {
        cancelAnimationFrame(raf);
        scroller.removeEventListener('wheel', onWheel);
        scroller.removeEventListener('scroll', onScroll);
    };
    probe.result = () => {
        const frames = samples.filter(sample => sample.kind === 'frame' && sample.scrolling);
        const deltas = frames.slice(1).map((frame, index) => frame.top - frames[index].top);
        const movingFrames = deltas.filter(delta => Math.abs(delta) > .5).length;
        const bothDirections = deltas.some(delta => delta > .5) && deltas.some(delta => delta < -.5);
        return {valid: !overflow && samples.every(sample => sample.valid) && trustedWheels >= 2 && trustedScrolls >= 3 && movingFrames >= 3 && bothDirections,
            trustedWheels, trustedScrolls, movingFrames, bothDirections, overflow, maximum, samples};
    };
    sample('initial', performance.now());
    if (!samples[0].valid) return {valid: false, reason: 'Invalid initial Select geometry or paint', samples};
    const scrollBox = box(scroller);
    const x = Math.round(scrollBox.x + scrollBox.width / 2), y = Math.round(scrollBox.y + scrollBox.height / 2);
    const hit = document.elementFromPoint(x, y);
    if (!hit || !scroller.contains(hit)) return {valid: false, reason: 'Wheel origin does not hit the option scroller', x, y};
    scroller.addEventListener('wheel', onWheel, {passive: true});
    scroller.addEventListener('scroll', onScroll, {passive: true});
    window[key] = probe;
    raf = requestAnimationFrame(onFrame);
    return {valid: true, x, y, maximum};
})().then(done, error => done({valid: false, error: String(error)}));
