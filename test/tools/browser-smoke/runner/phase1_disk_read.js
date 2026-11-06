// Only delivery is interrupted. Both disk GETs reach the real server; no
// response data, access flags, timers or service limits are changed.
if (window.__zxsPhase1DiskRead) throw new Error('Disk-read probe already installed');
const original = window.fetch;
const held = [];
const p = window.__zxsPhase1DiskRead = {
    reads: [], statusHeld: false, holdStatus: true, closed: false,
    failure: 'Browser smoke: phase-one disk response delivery interrupted',
    release(index, fail = false) {
        const record = p.reads[index];
        if (!record?.release) throw new Error('disk response is not held');
        record.fail = fail;
        record.release();
    },
    restore() {
        p.closed = true;
        p.holdStatus = false;
        window.fetch = original;
        for (const release of held) release();
        delete window.__zxsPhase1DiskRead;
    }
};
const pause = async record => {
    await new Promise(resolve => { record.release = resolve; held.push(resolve); });
};
window.fetch = async function(...args) {
    const input = args[0];
    const url = new URL(typeof input === 'string' ? input : input.url, location.href);
    const method = (args[1]?.method || input.method || 'GET').toUpperCase();
    const own = url.origin === location.origin && method === 'GET';
    const disk = own && url.pathname === '/api/file-manager/disks';
    const record = disk ? {status: null, body: null, delivered: false, fail: false} : null;
    if (record) p.reads.push(record);
    const response = await Reflect.apply(original, this, args);
    // One outstanding real health read excludes automatic recovery refreshes
    // from the explicit-button experiment. The original bytes are released.
    if (own && url.pathname === '/api/management/status' && p.holdStatus && !p.closed) {
        if (!response.ok) throw new Error('real management status failed during disk probe');
        p.statusHeld = true;
        await pause({});
    }
    if (!record) return response;
    record.status = response.status;
    record.body = await response.clone().json();
    if (!p.closed) await pause(record);
    record.delivered = true;
    if (record.fail && !p.closed) throw new TypeError(p.failure);
    return response;
};
return true;
