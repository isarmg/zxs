// Decode a real Chrome screenshot, not the G4 source image. Samples therefore
// fail if another layer covers the fill, or scrolling clips its visible corners.
const encoded = arguments[0], scene = arguments[1], done = arguments[arguments.length - 1];
(async () => {
    const bytes = Uint8Array.from(atob(encoded), value => value.charCodeAt(0));
    const url = URL.createObjectURL(new Blob([bytes], {type: 'image/png'}));
    try {
        const image = new Image(); image.src = url; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width = image.naturalWidth; canvas.height = image.naturalHeight;
        const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
        const {width, height} = canvas, pixels = context.getImageData(0, 0, width, height).data;
        const rgba = value => {
            const sample = document.createElement('canvas'); sample.width = sample.height = 1;
            const paint = sample.getContext('2d'); paint.fillStyle = value; paint.fillRect(0, 0, 1, 1);
            return [...paint.getImageData(0, 0, 1, 1).data];
        };
        const pixel = (x, y) => [...pixels.slice((y * width + x) * 4, (y * width + x) * 4 + 4)];
        const matches = (sample, expected) => sample.every((value, channel) => Math.abs(value - expected[channel]) <= 4);
        if (scene.kind === 'row') {
            const panel = rgba(scene.panel), fill = rgba(scene.fill);
            const corners = [[2, 2], [width - 3, 2], [width - 3, height - 3], [2, height - 3]].map(([x, y]) => pixel(x, y));
            const centers = [pixel(Math.floor(width / 2), 2), pixel(Math.floor(width / 2), height - 3)];
            return {valid: width > 36 && height >= 34 && !matches(panel, fill)
                && corners.every(value => matches(value, panel)) && centers.every(value => matches(value, fill)),
                kind: scene.kind, width, height, panel, fill, corners, centers};
        }
        if (scene.kind === 'search-focus') {
            const color = rgba(scene.color), page = rgba(scene.page), f = scene.frame;
            const sample = (x, y) => pixel(Math.floor(x * width / scene.clip.width), Math.floor(y * height / scene.clip.height));
            const edges = distance => [
                sample(f.x + f.width / 2, f.y - distance), sample(f.x + f.width + distance - 1, f.y + f.height / 2),
                sample(f.x + f.width / 2, f.y + f.height + distance - 1), sample(f.x - distance, f.y + f.height / 2),
            ];
            const ring = edges(4), gap = edges(2);
            // The old native rectangle painted these four diagonal outer pixels.
            // G4 leaves them page-colored while the curved ring remains visible.
            const corners = [sample(f.x - 4, f.y - 4), sample(f.x + f.width + 3, f.y - 4),
                sample(f.x + f.width + 3, f.y + f.height + 3), sample(f.x - 4, f.y + f.height + 3)];
            // A one-pixel diagonal can land entirely on antialiased edges.
            // Observe a bounded 2 CSS px diagonal band using distinct screenshot
            // pixel centers instead. Keep the original color tolerance and require
            // at least two pixels spanning both axes in EACH corner. Stop halfway
            // through the corner extent, before the input's text/caret interior.
            const scaleX = width / scene.clip.width, scaleY = height / scene.clip.height;
            const reach = scene.extent / 2;
            const curveDetails = [0, 1, 2, 3].map(corner => {
                const left = corner === 0 || corner === 3, top = corner < 2;
                const originX = f.x + (left ? 0 : f.width), originY = f.y + (top ? 0 : f.height);
                const x0 = Math.max(0, Math.floor((originX - (left ? 5 : reach)) * scaleX));
                const x1 = Math.min(width, Math.ceil((originX + (left ? reach : 5)) * scaleX));
                const y0 = Math.max(0, Math.floor((originY - (top ? 5 : reach)) * scaleY));
                const y1 = Math.min(height, Math.ceil((originY + (top ? reach : 5)) * scaleY));
                const hits = [];
                let inspected = 0;
                for (let y = y0; y < y1; y++) for (let x = x0; x < x1; x++) {
                    const dx = ((x + .5) / scaleX - originX) * (left ? 1 : -1);
                    const dy = ((y + .5) / scaleY - originY) * (top ? 1 : -1);
                    if (dx < -5 || dy < -5 || dx > reach || dy > reach || Math.abs(dx - dy) > 2) continue;
                    inspected++;
                    if (matches(pixel(x, y), color)) hits.push({x, y});
                }
                return {inspected, hits, xCount: new Set(hits.map(p => p.x)).size, yCount: new Set(hits.map(p => p.y)).size};
            });
            const curves = curveDetails.map(corner => corner.hits.length);
            const checks = {
                distinctFocus: !matches(color, page), corners: corners.every(p => matches(p, page)),
                gap: gap.every(p => matches(p, page)),
                ring: ring.every(p => matches(p, scene.focused ? color : page)),
                curvedRing: curveDetails.every(corner => scene.focused
                    ? corner.hits.length >= 2 && corner.xCount >= 2 && corner.yCount >= 2
                    : corner.hits.length === 0),
            };
            return {valid: Object.values(checks).every(Boolean), kind: scene.kind, focused: scene.focused,
                checks, width, height, ring, gap, corners, curves, curveDetails,
                frame: f, extent: scene.extent, clip: scene.clip};
        }
        if (scene.kind === 'focus') {
            const expected = rgba(scene.color), strip = Math.ceil(scene.ring + 1);
            const count = (start, end) => {
                let hits = 0;
                for (let x = start; x < end; x++) for (let y = Math.floor(height / 2) - 3; y <= Math.floor(height / 2) + 3; y++) {
                    if (matches(pixel(x, y), expected)) hits++;
                }
                return hits;
            };
            const left = count(0, strip), right = count(width - strip, width);
            return {valid: left >= 4 && right >= 4, kind: scene.kind, left, right, expected};
        }
        return {valid: false, reason: 'unknown screenshot scene'};
    } finally {
        URL.revokeObjectURL(url);
    }
})().then(done, error => done({valid: false, error: String(error)}));
