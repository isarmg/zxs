#!/usr/bin/env python3
"""Compile the actual Web generator contract and reject retired lower-level APIs."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

from pathlib import Path
import subprocess
import tempfile
import functools
import http.server
import json
import os
import threading
import shutil
import re

ROOT = Path(__file__).resolve().parents[2]
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target/web-bindgen-scope-check'))


def cargo(*arguments, capture=False):
    return subprocess.run(['cargo', *arguments], cwd=ROOT, check=not capture,
                          capture_output=capture, text=capture)


def browser_configuration():
    """Local probes may opt out; CI/release verification must execute Chromium."""
    browser = os.environ.get('ZXS_CHROMIUM')
    if not browser:
        if os.environ.get('ZXS_REQUIRE_BROWSER') == '1':
            raise RuntimeError('ZXS_CHROMIUM is required for browser contract verification')
        return None
    if not shutil.which(browser):
        raise RuntimeError(f'ZXS_CHROMIUM is not executable: {browser}')
    return browser


def browser_check(output):
    """Execute actual Chromium, never a Node/native substitute."""
    browser = browser_configuration()
    if not browser:
        return 'browser execution not requested'
    (output / 'index.html').write_text(
        (ROOT / 'test/wasm-binding-macros/browser-runtime.html').read_text())
    class Handler(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_args):
            pass
    handler = functools.partial(Handler, directory=str(output))
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix='zxs-browser-futures-') as profile:
            result = subprocess.run([
                browser, '--headless', '--no-first-run', '--no-default-browser-check',
                '--user-data-dir=' + profile, '--virtual-time-budget=10000', '--dump-dom',
                f'http://127.0.0.1:{server.server_port}/index.html',
            ], text=True, capture_output=True, timeout=60)
            assert result.returncode == 0, result.stderr
            assert '"status":"passed"' in result.stdout, result.stdout + result.stderr
            body = result.stdout.split('<body>', 1)[1].split('</body>', 1)[0]
            checks = json.loads(body)['checks']
            expected = {
                'generated async export', 'generated async import', 'JsFuture and Promise IntoFuture',
                'future_to_promise resolution and rejection', 'spawn_local next microtask',
                'empty Unicode and large strings', 'async class method and retained state',
                'task dispatch Send and local futures', 'task dispatch initialization and tick',
            }
            assert len(checks) == len(expected) and set(checks) == expected, checks
            return f'{len(checks)} actual Chromium async runtime checks passed'
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def check():
    browser_configuration()  # Fail before compiling if required browser setup is missing.
    (ROOT / 'target').mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='web-bindgen-scope-', dir=ROOT / 'target') as temporary:
        work = Path(temporary)
        (work / 'src').mkdir()
        manifest = work / 'Cargo.toml'
        manifest.write_text('[package]\nname="web-bindgen-scope"\nversion="0.0.0"\nedition="2021"\n'
                            '[workspace]\n[dependencies]\nwasm-bindgen-cli-support={path="' +
                            str(ROOT / 'src/foundation/web-runtime/wasm-bindgen-cli-support') + '"}\n')
        source = work / 'src/lib.rs'
        source.write_text('pub fn generator() { let _ = wasm_bindgen_cli_support::Bindgen::new(); }')
        cargo('generate-lockfile', '--offline', '--manifest-path', str(manifest))
        command = ['check', '--locked', '--offline', '--manifest-path', str(manifest),
                   '--target-dir', str(TARGET)]
        cargo(*command)
        retired = {
            'encode_into': 'pub fn old() { wasm_bindgen_cli_support::Bindgen::new().encode_into(wasm_bindgen_cli_support::EncodeInto::Test); }',
            'typescript': 'pub fn old() { wasm_bindgen_cli_support::Bindgen::new().typescript(true); }',
            'typed_array_declarations': 'pub fn old() { wasm_bindgen_cli_support::Bindgen::new().ts_typed_array_buffers(true); }',
            'typescript_output': 'pub fn old(output: wasm_bindgen_cli_support::Output) { output.ts(); }',
            'backend_start_output': 'pub fn old(output: wasm_bindgen_cli_support::Output) { output.start(); }',
            'wasm2es6js': 'use wasm_bindgen_cli_support::wasm2es6js;',
        }
        for name in ['nodejs', 'nodejs_module', 'deno', 'bundler', 'no_modules', 'module', 'browser', 'web']:
            retired[name] = f'pub fn old() {{ wasm_bindgen_cli_support::Bindgen::new().{name}(true); }}'
        retired['no_modules_global'] = 'pub fn old() { wasm_bindgen_cli_support::Bindgen::new().no_modules_global("old"); }'
        for name, code in retired.items():
            source.write_text(code)
            result = cargo(*command, capture=True)
            if not result.returncode or not any(error in result.stderr for error in ('error[E0599]', 'error[E0432]')):
                raise AssertionError(f'{name} was accepted or failed for another reason: {result.stderr}')

        fixture = work / 'fixture'
        (fixture / 'src').mkdir(parents=True)
        fixture_manifest = fixture / 'Cargo.toml'
        fixture_manifest.write_text('[package]\nname="web-binding-contract-fixture"\nversion="0.0.0"\nedition="2021"\n'
                                    '[workspace]\n[lib]\ncrate-type=["cdylib"]\n[dependencies]\nwasm-bindgen={path="' +
                                    str(ROOT / 'src/foundation/web-runtime/wasm-bindgen') + '"}\njs-sys={path="' +
                                    str(ROOT / 'src/foundation/web-runtime/js-sys') + '",default-features=false,features=["std"]}\ntask-dispatch={path="' +
                                    str(ROOT / 'src/foundation/concurrency/task-dispatch') + '",features=["wasm-bindgen"]}\n')
        (fixture / 'src/lib.rs').write_text((ROOT / 'test/tools/web-bindings/fixtures/contract.rs').read_text() + '\n' +
                                              (ROOT / 'test/wasm-binding-macros/async-runtime.rs').read_text() + '\n' +
                                              (ROOT / 'test/task-dispatch/browser.rs').read_text())
        cargo('generate-lockfile', '--offline', '--manifest-path', str(fixture_manifest))
        target = TARGET
        cargo('build', '--locked', '--offline', '--release', '--target', 'wasm32-unknown-unknown',
              '--manifest-path', str(fixture_manifest), '--target-dir', str(target))
        output = work / 'output'
        cargo('run', '--locked', '--offline', '-p', 'web-bindings', '--',
              str(target / 'wasm32-unknown-unknown/release/web_binding_contract_fixture.wasm'),
              '--target', 'web', '--no-typescript', '--out-name', 'fixture', '--out-dir', str(output))
        assert {p.name for p in output.iterdir()} == {'fixture.js', 'fixture_bg.wasm'}
        js = (output / 'fixture.js').read_text()
        for required in ['get value()', 'set value(', 'class Widget', 'Object.freeze(',
                         '__wbindgen_enum_Mode', '文件 Ω', 'function mode_roundtrip(',
                         'function message_roundtrip(', 'function unicode_roundtrip(',
                         'function async_roundtrip(', 'class AsyncCounter']:
            assert required in js, f'missing runtime binding: {required}'
        for forbidden in ['DeclarationMustNotLeak', '@ts-self-types', 'addToLibrary(', 'require(', 'Deno.', "if (!('encodeInto' in cachedTextEncoder))", 'cachedTextEncoder.encodeInto = function']:
            assert forbidden not in js, f'unsupported output returned: {forbidden}'
        for name in ['WidgetFinalization', 'AsyncCounterFinalization', 'CLOSURE_DTORS']:
            assert re.search(r'const\s+' + name + r'\s*=\s*new FinalizationRegistry\(', js), name
        for forbidden in ["typeof FinalizationRegistry", "register: () => {}",
                          "typeof WebAssembly.instantiateStreaming", "Falling back to",
                          "expectedResponseType", "module.arrayBuffer()"]:
            assert forbidden not in js, f'legacy browser fallback returned: {forbidden}'
        subprocess.run(['node', str(ROOT / 'test/tools/web-bindings/runtime/modern-loader.mjs'),
                        str(output)], cwd=ROOT, check=True)
        browser_result = browser_check(output)
        print(f'Web/no-TypeScript contract: runtime fixture and {len(retired)} negative API groups passed; {browser_result}')


if __name__ == '__main__':
    check()
