// Observe production fetch calls. Forward actual status, headers and body bytes.
// Hold the real disk-list body, rather than just delaying its response headers.
const original = window.fetch;
let release;
const gate = new Promise(resolve => release = resolve);
const probe = window.__zxsAdminRefresh = {
    count: 0, held: false, active: 0, peak: 0, paths: [], failures: [],
    atHold: null, framesPassed: false, release,
    restore: () => { release(); window.fetch = original; delete window.__zxsAdminRefresh; }
};
const managed = path => path === '/api/admin/disks'
    || path.startsWith('/api/admin/disks/') || path.startsWith('/api/admin/storage/')
    || path.startsWith('/api/setup/') || ['/api/workspaces/capacity', '/api/capabilities'].includes(path);
window.fetch = async function(...args) {
    const request = args[0];
    const url = new URL(typeof request === 'string' ? request : request.url, location.href);
    const method = request.method || args[1]?.method || 'GET';
    if (method !== 'GET' || url.origin !== location.origin || !managed(url.pathname)) {
        return Reflect.apply(original, this, args);
    }
    probe.paths.push(url.pathname);
    probe.active++;
    probe.peak = Math.max(probe.peak, probe.active);
    let finished = false;
    const finish = () => {
        if (!finished) { finished = true; probe.active--; }
    };
    try {
        const response = await Reflect.apply(original, this, args);
        if (!response.ok) probe.failures.push({path: url.pathname, status: response.status});
        if (!response.body) { finish(); return response; }
        const held = url.pathname === '/api/admin/disks';
        if (held) {
            probe.count++;
            probe.held = true;
            probe.atHold = probe.paths.length;
            requestAnimationFrame(() => requestAnimationFrame(() => probe.framesPassed = true));
        }
        const reader = response.body.getReader();
        return new Response(new ReadableStream({
            async pull(controller) {
                try {
                    if (held) await gate;
                    const part = await reader.read();
                    if (part.done) {
                        finish();
                        reader.releaseLock();
                        controller.close();
                    } else {
                        controller.enqueue(part.value);
                    }
                } catch (error) {
                    finish();
                    controller.error(error);
                }
            },
            async cancel(reason) {
                try { await reader.cancel(reason); } finally { finish(); }
            }
        }), {status: response.status, statusText: response.statusText, headers: response.headers});
    } catch (error) {
        finish();
        throw error;
    }
};
return true;
