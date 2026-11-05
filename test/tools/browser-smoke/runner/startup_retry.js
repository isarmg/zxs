// Read the actual terminal startup control. No DOM, focus or style changes.
const node = document.querySelector('#zxs-startup-error button');
if (!node) return {valid: false, reason: 'missing retry'};
const css = getComputedStyle(node), box = node.getBoundingClientRect();
const borders = ['Top', 'Right', 'Bottom', 'Left'].map(side => parseFloat(css[`border${side}Width`]));
const corners = ['TopLeft', 'TopRight', 'BottomRight', 'BottomLeft'].map(corner => parseFloat(css[`border${corner}Radius`]));
const focused = document.activeElement === node, keyboard = node.matches(':focus-visible');
const outline = parseFloat(css.outlineWidth) > 0 && !['none', 'hidden'].includes(css.outlineStyle);
const visible = box.width > 0 && box.height > 0 && css.display !== 'none' && css.visibility === 'visible' && Number(css.opacity) > 0;
const state = {expected, visible, borders, corners, focused, keyboard, hovered: node.matches(':hover'),
    background: css.backgroundColor, outline: css.outlineStyle, outlineWidth: css.outlineWidth};
const frame = borders.every(width => width === 0) && corners.every(radius => radius > 0)
    && css.appearance === 'none' && css.boxShadow === 'none';
const focus = expected === 'keyboard' ? focused && keyboard && outline && css.outlineStyle === 'solid'
    && parseFloat(css.outlineWidth) >= 2 : !focused && !keyboard && !outline;
return {...state, valid: visible && frame && focus && ['rest', 'hover', 'keyboard'].includes(expected)
    && (expected !== 'hover' || state.hovered) && (expected !== 'rest' || !state.hovered)};
