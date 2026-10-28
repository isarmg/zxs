// Faults enter through native exception/Response values, never Rust UI state.
const originalFetch = window.fetch;
const originalSetTimeout = window.setTimeout;
const originalClearTimeout = window.clearTimeout;
const timers = new Map();
const pending = new Set();
let lastDeadline;
const state = window.__zxsErrorFeedback = {
    mode: 'pass', reads: 0, aborted: 0, polls: 0, writes: 0, operation: null,
    recover: false, pollPaths: [], timeout: null,
    restore() {
        window.fetch = originalFetch;
        window.setTimeout = originalSetTimeout;
        window.clearTimeout = originalClearTimeout;
        for (const release of pending) release();
        pending.clear();
        timers.clear();
    },
};
window.setTimeout = function(callback, delay, ...args) {
    const id = originalSetTimeout.call(this, callback, delay, ...args);
    if (delay === 120000) {
        timers.set(id, callback);
        lastDeadline = id;
    }
    return id;
};
window.clearTimeout = function(id) {
    timers.delete(id);
    return originalClearTimeout.call(this, id);
};
window.fetch = async function(...args) {
    const request = args[0];
    const url = new URL(typeof request === 'string' ? request : request.url, location.href);
    const method = request.method || 'GET';
    if (/^\/api\/file-manager\/disks\/[^/]+\/entries$/.test(url.pathname)) {
        state.reads++;
        switch (state.mode) {
            case 'error': throw new Error('Smoke structured Error detail');
            case 'network': throw new TypeError('Smoke fetch or policy rejection');
            case 'dom': throw new DOMException('Smoke security exception detail', 'SecurityError');
            case 'unowned-abort': throw new DOMException('Smoke unowned abort detail', 'AbortError');
            case 'literal': throw 'Smoke literal rejection';
            case 'body': return new Response(new ReadableStream({start(controller) {
                controller.error(new DOMException('Smoke response body detail', 'NetworkError'));
            }}), {status: 200});
            case 'cancel':
            case 'timeout':
            case 'body-timeout':
            case 'timeout-undefined':
            case 'timeout-race': {
                const mode = state.mode;
                const deadline = lastDeadline;
                state.timeout = () => {
                    const callback = timers.get(deadline);
                    if (!callback) throw new Error('request deadline already released');
                    originalClearTimeout(deadline);
                    callback();
                };
                const waitForAbort = reject => {
                    const release = () => {
                        request.signal.removeEventListener('abort', abort);
                        reject(new DOMException('Smoke fixture cleanup', 'AbortError'));
                    };
                    const abort = () => {
                        pending.delete(release);
                        state.aborted++;
                        reject(mode === 'timeout-race' ? new TypeError('Smoke unrelated timeout race')
                            : mode === 'timeout-undefined' ? undefined : request.signal.reason);
                    };
                    pending.add(release);
                    request.signal.addEventListener('abort', abort, {once: true});
                };
                if (mode === 'body-timeout') {
                    return new Response(new ReadableStream({start(controller) {
                        waitForAbort(error => controller.error(error));
                    }}), {status: 200});
                }
                return new Promise((_, reject) => waitForAbort(reject));
            }
        }
    }
    if (state.mode === 'mutation' && method === 'POST'
        && /^\/api\/file-manager\/files\/[^/]+\/metadata$/.test(url.pathname)) {
        state.writes++;
        if (!state.operation) {
            state.operation = (await request.clone().json()).operation_id;
            const response = await originalFetch.apply(this, args);
            if (!response.ok) throw new Error('real metadata mutation was not accepted');
            state.mutationStatus = response.status;
            // Publication really succeeded; a bare AbortError cannot release its fence.
            throw new DOMException('Smoke mutation reply lost after publication', 'AbortError');
        }
    }
    if (state.operation && url.pathname === '/api/operations/' + state.operation) {
        state.polls++;
        state.pollPaths.push(url.pathname);
        if (!state.recover) {
            return new Response(JSON.stringify({code: 'RECOVERY_REQUIRED', message: 'Smoke poll failure',
                retryable: true, request_id: 'smoke-poll-' + state.polls}),
                {status: 503, headers: {'Content-Type': 'application/json'}});
        }
    }
    return originalFetch.apply(this, args);
};
return true;
