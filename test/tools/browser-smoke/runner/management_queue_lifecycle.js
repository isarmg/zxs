// Faults exercise the production WASM API boundary, never private Rust state.
const originalFetch = window.fetch;
const OriginalRequest = window.Request;
const originalSetTimeout = window.setTimeout;
const originalClearTimeout = window.clearTimeout;
const requests = new WeakMap();
const deadlines = new Map();
let awaitingDeadline = null, release, oldCredential, newCredential;
const gate = new Promise(resolve => release = resolve);
const p = window.__zxsManagementQueue = {
    phase: 'old', mode: 'hold', held: false, delivered: false, framesPassed: false,
    records: [], lateOldStarts: 0, rateLimitCalls: 0, active: null, timedOut: null,
    newCredentialCaptured: false, newCredentialDistinct: false, newCredentialChecks: 0, credentialChanged: false,
    untracked: 0, lateUnauthorized: false, release,
    timeoutQueued() {
        const candidates = p.records.filter(record => record.phase === 'old'
            && record.path === '/api/admin/disks/readmission' && record.method === 'GET'
            && !record.started && !record.signal.aborted && deadlines.has(record.deadline));
        if (candidates.length !== 1) throw new Error('expected one exact queued readmission request');
        const record = candidates[0];
        const callback = deadlines.get(record.deadline);
        originalClearTimeout(record.deadline);
        callback();
        p.timedOut = record;
    },
    restore() {
        release();
        window.fetch = originalFetch;
        window.Request = OriginalRequest;
        window.setTimeout = originalSetTimeout;
        window.clearTimeout = originalClearTimeout;
        delete window.__zxsManagementQueue;
    }
};
// Production builds Request and then its deadline without yielding. Observe the
// native constructor transparently; the original AbortSignal identifies the owner.
window.Request = class extends OriginalRequest {
    constructor(...args) {
        super(...args);
        if (!args[1]?.signal) return;
        if (awaitingDeadline) throw new Error('request was replaced before its deadline was bound');
        const record = {phase: p.phase, path: new URL(this.url).pathname, method: this.method,
            signal: args[1].signal, started: false, deadline: null};
        requests.set(this, record);
        p.records.push(record);
        awaitingDeadline = record;
    }
};
window.setTimeout = function(callback, delay, ...args) {
    const id = originalSetTimeout.call(this, callback, delay, ...args);
    if (delay === 120000) {
        if (!awaitingDeadline) throw new Error('request deadline has no exact Request owner');
        awaitingDeadline.deadline = id;
        deadlines.set(id, callback);
        awaitingDeadline = null;
    }
    return id;
};
window.clearTimeout = function(id) {
    deadlines.delete(id);
    return originalClearTimeout.call(this, id);
};
const discovery = '/api/admin/storage/discovered-disks';
const failure = (status, code, message) => new Response(JSON.stringify({error: {
    code, message, retryable: status === 429, request_id: 'smoke-management-queue'
}}), {status, headers: {'Content-Type': 'application/json'}});
window.fetch = async function(...args) {
    const request = args[0];
    const url = new URL(request.url, location.href);
    const method = request.method || 'GET';
    const record = requests.get(request);
    if (record) record.started = true;
    const forward = async () => {
        const response = await Reflect.apply(originalFetch, this, args);
        if (record) record.deliveredStatus = response.status;
        return response;
    };
    if (url.origin !== location.origin || method !== 'GET') return forward();
    if (record?.phase === 'new' &&
        (url.pathname === '/api/file-manager/disks' || url.pathname.startsWith('/api/admin/'))) {
        const credential = request.headers.get('Authorization');
        if (url.pathname === '/api/file-manager/disks' && !newCredential) {
            newCredential = credential; // Closure only; never reported or persisted.
            p.newCredentialCaptured = !!credential;
            p.newCredentialDistinct = !!credential && !!oldCredential && credential !== oldCredential;
        }
        if (newCredential) {
            p.newCredentialChecks++;
            if (credential !== newCredential) p.credentialChanged = true;
        }
    }
    if (url.pathname.startsWith('/api/admin/storage/') || url.pathname.startsWith('/api/admin/disks')) {
        if (!record) p.untracked++;
        if (record?.phase === 'old' && p.phase !== 'old') p.lateOldStarts++;
    }
    if (url.pathname !== discovery) return forward();
    if (p.mode === 'rate-limit') {
        p.rateLimitCalls++;
        if (record) record.deliveredStatus = 429;
        return failure(429, 'RESOURCE_LIMIT_EXCEEDED', 'management operation capacity reached');
    }
    if (p.mode !== 'hold') return forward();
    p.mode = 'pass';
    if (!record) throw new Error('discovery has no exact captured Request');
    p.active = record;
    oldCredential = request.headers.get('Authorization');
    if (!oldCredential) throw new Error('fixture needs the fresh login Bearer for the late-401 comparison');
    const response = await forward();
    if (!response.ok) throw new Error('real discovery seed failed');
    p.held = true;
    await gate; // Hold headers before Rust sees the old 401, and deliberately ignore abort.
    p.delivered = true;
    requestAnimationFrame(() => requestAnimationFrame(() => p.framesPassed = true));
    if (p.lateUnauthorized) {
        record.deliveredStatus = 401;
        return failure(401, 'UNAUTHORIZED', 'smoke-old-management-401');
    }
    return response;
};
return true;
