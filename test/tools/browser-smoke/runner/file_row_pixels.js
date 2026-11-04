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
