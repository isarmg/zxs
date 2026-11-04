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
            const curves = [0, 1, 2, 3].map(corner => {
                let hits = 0;
                for (let d = -4; d <= scene.extent / 2 + 2; d += .5) {
                    const x = corner === 0 || corner === 3 ? f.x + d : f.x + f.width - 1 - d;
                    const y = corner < 2 ? f.y + d : f.y + f.height - 1 - d;
                    if (matches(sample(x, y), color)) hits++;
                }
                return hits;
            });
            const checks = {
                distinctFocus: !matches(color, page), corners: corners.every(p => matches(p, page)),
                gap: gap.every(p => matches(p, page)),
                ring: ring.every(p => matches(p, scene.focused ? color : page)),
                curvedRing: !scene.focused || curves.every(hits => hits >= 2),
            };
            return {valid: Object.values(checks).every(Boolean), kind: scene.kind, focused: scene.focused,
                checks, width, height, ring, gap, corners, curves};
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
