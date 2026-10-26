// Execute the actual generated Web module in Node's JS/WASM engine. No HTTP
// listener or browser is started; this does not substitute for Chromium QA.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {join} from 'node:path';

const output = process.argv[2];
assert.ok(output, 'generated fixture directory is required');
const source = await readFile(join(output, 'fixture.js'), 'utf8');
const bytes = await readFile(join(output, 'fixture_bg.wasm'));
const moduleURL = `data:text/javascript;base64,${Buffer.from(source).toString('base64')}`;
const fresh = name => import(`${moduleURL}#${name}`);
const response = (mime = 'application/wasm') => new Response(bytes, {
    headers: {'Content-Type': mime},
});
const checks = [];

const registry = globalThis.FinalizationRegistry;
try {
    globalThis.FinalizationRegistry = undefined;
    await assert.rejects(fresh('missing-finalization-registry'), error =>
        error instanceof TypeError && /FinalizationRegistry/.test(error.message));
    checks.push('missing FinalizationRegistry rejects module import');
} finally {
    globalThis.FinalizationRegistry = registry;
}

const missingEncoder = await fresh('missing-encode-into');
await missingEncoder.default({module_or_path: bytes});
const encodeInto = TextEncoder.prototype.encodeInto;
try {
    TextEncoder.prototype.encodeInto = undefined;
    assert.throws(() => missingEncoder.unicode_roundtrip('文件 Ω'), error =>
        error instanceof TypeError && /encodeInto/.test(error.message));
    checks.push('missing encodeInto fails without an encode-copy polyfill');
} finally {
    TextEncoder.prototype.encodeInto = encodeInto;
}

const streaming = WebAssembly.instantiateStreaming;
const instantiate = WebAssembly.instantiate;
let byteFallbackCalls = 0;
try {
    // A Response must never be retried through the byte/Module API, including
    // capability absence and real host/network/MIME/compile failures.
    WebAssembly.instantiate = (...args) => {
        byteFallbackCalls++;
        return Reflect.apply(instantiate, WebAssembly, args);
    };
    WebAssembly.instantiateStreaming = undefined;
    const missing = await fresh('missing-streaming');
    await assert.rejects(missing.default({module_or_path: response()}), error =>
        error instanceof TypeError && /instantiateStreaming/.test(error.message));
    checks.push('missing instantiateStreaming rejects initialization');

    WebAssembly.instantiateStreaming = streaming;
    const wrongMime = await fresh('wrong-mime');
    await assert.rejects(wrongMime.default({module_or_path: response('text/plain')}), TypeError);
    checks.push('incorrect MIME is rejected without byte fallback');

    const rejectedHost = await fresh('host-rejection');
    const hostError = new Error('deliberate streaming failure');
    WebAssembly.instantiateStreaming = async () => { throw hostError; };
    await assert.rejects(rejectedHost.default({module_or_path: response()}),
        error => error === hostError);
    checks.push('streaming error identity propagates unchanged');

    WebAssembly.instantiateStreaming = streaming;
    const invalid = await fresh('invalid-wasm');
    await assert.rejects(invalid.default({module_or_path: new Response('invalid wasm', {
        headers: {'Content-Type': 'application/wasm'},
    })}), WebAssembly.CompileError);
    checks.push('invalid WASM compile failure propagates');

    const httpFailure = await fresh('http-failure');
    await assert.rejects(httpFailure.default({module_or_path: new Response('unavailable', {
        status: 503, statusText: 'Service Unavailable',
    })}), /failed to fetch Wasm: 503 Service Unavailable/);
    checks.push('HTTP failure retains status and status text');
    assert.equal(byteFallbackCalls, 0, 'failed Response loading attempted a byte fallback');
} finally {
    WebAssembly.instantiateStreaming = streaming;
    WebAssembly.instantiate = instantiate;
}

for (const [name, input] of [
    ['streaming Response', response()],
    ['explicit bytes', bytes],
    ['explicit Module', new WebAssembly.Module(bytes)],
]) {
    const bindings = await fresh(name);
    await bindings.default({module_or_path: input});
    assert.equal(bindings.unicode_roundtrip('文件 Ω'), '文件 Ω');
    assert.equal(await bindings.async_roundtrip('async Ω'), 'async Ω');
    const widget = new bindings.Widget();
    assert.equal(widget.rename('retained Ω'), 'retained Ω');
    assert.equal(widget.value, 'retained Ω');
    widget.free();
    checks.push(`${name} initializes real WASM and sync/async/class bindings`);
}
console.log(`PASS ${checks.length} generated Web-loader checks in Node (not browser acceptance)`);
