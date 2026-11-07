// Synchronous computed-style observation of actual text controls. This probe
// never focuses, enables, styles, or reads the value of a product control.
const layoutWidth = document.documentElement.clientWidth;
const forced = matchMedia('(forced-colors: active)').matches;
const excludedTypes = new Set(['button', 'checkbox', 'color', 'file', 'hidden', 'image', 'radio', 'range', 'reset', 'submit']);
const candidates = [...document.querySelectorAll('input.zxs-input,textarea.zxs-input')];
const textControls = candidates.filter(node => node.tagName === 'TEXTAREA' || !excludedTypes.has(node.type));
const disclosed = node => {
    for (let ancestor = node.parentElement; ancestor; ancestor = ancestor.parentElement) {
        if (ancestor.tagName === 'DETAILS' && !ancestor.open) {
            const summary = [...ancestor.children].find(child => child.tagName === 'SUMMARY');
            if (!summary?.contains(node)) return false;
        }
    }
    return true;
};
const alpha = color => {
    if (color === 'transparent') return 0;
    const explicit = color.match(/(?:\/|,\s*(?=[^,]*\)$))\s*([\d.]+)(%)?\s*\)$/);
    // Legacy rgb() has three comma-separated channels but no alpha channel.
    if (!explicit || (!color.includes('/') && !color.startsWith('rgba(') && !color.startsWith('hsla('))) return 1;
    return Number(explicit[1]) / (explicit[2] ? 100 : 1);
};
// Zero opacity is a failed displayed control, not an excuse to omit it. Closed
// disclosures and display/visibility-hidden controls are outside this scene.
const visible = textControls.filter(node => disclosed(node) && node.checkVisibility({opacityProperty: false, visibilityProperty: true}));
const controls = visible.map((node, index) => {
    const disabled = node.disabled;
    const effectiveDisabled = node.matches(':disabled');
    const focusVisible = node.matches(':focus-visible');
    const anonymous = node.closest('.authentication-shell .authentication-card') !== null;
    const searchFrame = node.closest('.zxs-file-search');
    const search = searchFrame !== null;
    const storageDirectory = node.closest('.zxs-storage-activation') !== null;
    const css = getComputedStyle(node);
    const rect = node.getBoundingClientRect();
    const borders = Object.fromEntries(['Top', 'Right', 'Bottom', 'Left'].map(side => [side.toLowerCase(), {
        width: parseFloat(css[`border${side}Width`]),
        style: css[`border${side}Style`],
        color: css[`border${side}Color`],
    }]));
    const outline = {width: parseFloat(css.outlineWidth), style: css.outlineStyle, color: css.outlineColor, offset: parseFloat(css.outlineOffset)};
    const outlineVisible = outline.width > 0 && !['none', 'hidden'].includes(outline.style) && alpha(outline.color) > 0;
    const contour = {
        extent: css.getPropertyValue('--zxs-contour-extent').trim(),
        extentOverride: node.getAttribute('data-zxs-contour-extent'),
        foreground: css.getPropertyValue('--zxs-contour-foreground').trim(),
        state: node.getAttribute('data-zxs-contour-state'),
        decorations: node.querySelectorAll('[data-zxs-contour-decoration]').length,
    };
    const markerSide = css.direction === 'rtl' ? 'right' : 'left';
    const marker = borders[markerSide];
    const markerVisible = marker.width === 2 && marker.style === 'solid' && alpha(marker.color) > 0;
    const opacity = Number(css.opacity);
    const frameCss = search ? getComputedStyle(searchFrame) : null;
    const frameRect = search ? searchFrame.getBoundingClientRect() : null;
    const frameOutline = search && parseFloat(frameCss.outlineWidth) > 0
        && !['none', 'hidden'].includes(frameCss.outlineStyle) && alpha(frameCss.outlineColor) > 0;
    const searchSurface = search ? {
        label: searchFrame.tagName === 'LABEL' && node.parentElement === searchFrame,
        singleInput: searchFrame.querySelectorAll('input').length === 1,
        noPermanentText: searchFrame.textContent.trim() === '',
        accessibleName: !!node.getAttribute('aria-label') && node.getAttribute('aria-label') === node.getAttribute('placeholder'),
        fullWidth: Math.abs(rect.left - frameRect.left - parseFloat(frameCss.borderLeftWidth)) <= 1
            && Math.abs(frameRect.right - rect.right - parseFloat(frameCss.borderRightWidth)) <= 1,
        fullHeight: Math.abs(rect.top - frameRect.top - parseFloat(frameCss.borderTopWidth)) <= 1
            && Math.abs(frameRect.bottom - rect.bottom - parseFloat(frameCss.borderBottomWidth)) <= 1,
        noBorder: ['Top', 'Right', 'Bottom', 'Left'].every(side => parseFloat(frameCss[`border${side}Width`]) === 0),
        noRectangle: !frameOutline || (forced && focusVisible),
        keyboardFocus: !focusVisible || (forced ? frameOutline && frameCss.outlineStyle === 'solid' && parseFloat(frameCss.outlineWidth) >= 2 : (searchFrame.getAttribute('data-zxs-contour-state') === 'paint'
                && frameCss.maskImage === 'none' && frameCss.clipPath === 'none'
                && frameCss.borderImageSource.startsWith('url("blob:')
                && parseFloat(frameCss.borderImageOutset) >= 5)),
    } : null;
    const checks = {
        connected: node.isConnected,
        noRectangle: ['top', 'right', 'bottom', 'left'].every(side => borders[side].width === (!search && side === markerSide ? 2 : 0)),
        bottomStyle: borders.bottom.style === 'none',
        bottomWidth: borders.bottom.width === 0,
        sharedRowHeight: node.tagName === 'TEXTAREA' ? rect.height >= parseFloat(css.getPropertyValue('--zxs-compact-row-height')) : rect.height === parseFloat(css.getPropertyValue('--zxs-compact-row-height')),
        searchSurface: !search || Object.values(searchSurface).every(Boolean),
        focusIndicator: !outlineVisible && (search || (marker.style === 'solid' && (focusVisible ? markerVisible : alpha(marker.color) === 0))),
        noImages: css.backgroundImage === 'none' && css.borderImageSource === 'none',
        noShadow: css.boxShadow === 'none',
        transparentBackground: alpha(css.backgroundColor) === 0,
        noContour: parseFloat(contour.extent) === 0 && parseFloat(contour.extentOverride ?? contour.extent) === 0 && !['paint', 'clip'].includes(contour.state),
        noForeground: Number(contour.foreground) === 0 && contour.decorations === 0,
        positiveSize: rect.width > 0 && rect.height > 0,
        withinViewport: rect.left >= -1 && rect.right <= layoutWidth + 1,
        disabledUnchanged: node.disabled === disabled && node.matches(':disabled') === effectiveDisabled,
        disabledAppearance: effectiveDisabled ? opacity > 0 && opacity < 1 : opacity === 1,
    };
    const failures = Object.entries(checks).filter(([, valid]) => !valid).map(([name]) => name);
    return {
        valid: failures.length === 0, failures, checks, index,
        tag: node.tagName, type: node.type, name: node.getAttribute('name'), label: node.getAttribute('aria-label'),
        disabled, effectiveDisabled, focusVisible, forced, anonymous, search, storageDirectory, searchSurface, opacity,
        rect: {left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom, width: rect.width, height: rect.height},
        borders, outline, outlineVisible, markerSide, markerVisible, contour,
        backgroundColor: css.backgroundColor, backgroundImage: css.backgroundImage,
        borderImageSource: css.borderImageSource, boxShadow: css.boxShadow,
    };
});
return {
    valid: controls.length > 0 && controls.every(control => control.valid),
    viewport: {clientWidth: layoutWidth},
    counts: {candidates: candidates.length, excludedTypes: candidates.length - textControls.length, hidden: textControls.length - visible.length, visible: controls.length},
    controls,
};
