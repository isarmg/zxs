#!/usr/bin/env python3
"""Prove the CSR/template contract and reject removed server-only APIs/features."""
from pathlib import Path
import os
import hashlib
import json
import shutil
import re
import subprocess
import tempfile
import tomllib

import runpy

ROOT = Path(__file__).resolve().parents[2]
rebase_dependency_paths = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['rebase_dependency_paths']
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target/csr-template-check'))


def cargo(*args, capture=False):
    return subprocess.run(['cargo', *args], cwd=ROOT, check=not capture,
                          capture_output=capture, text=capture)


def check_browser_futures_features():
    """Inspect the actual browser build graph, excluding native and test-only edges."""
    result = cargo('tree', '--locked', '--offline', '-p', 'web-app',
                   '--target', 'wasm32-unknown-unknown', '--edges', 'normal,build',
                   '--prefix', 'none', '--format', '{p}|{f}', capture=True)
    if result.returncode:
        raise AssertionError('browser dependency graph failed: ' + result.stderr)
    features = [set(match.group(1).split(',')) for line in result.stdout.splitlines()
                if (match := re.match(r'^futures v[^|]*\|([^\s]*)', line))]
    forbidden = {'executor', 'thread-pool', 'io', 'sink'}
    if not features or any(forbidden & selected for selected in features):
        raise AssertionError(f'browser futures acquired unused capabilities: {features}')


def check_reactive_units():
    """Compile the retained unit fixtures with their own test dependencies."""
    with tempfile.TemporaryDirectory(prefix='reactive-unit-') as temporary:
        snapshot = Path(temporary)
        shutil.copyfile(ROOT / 'rust-toolchain.toml', snapshot / 'rust-toolchain.toml')
        for relative in ('src/foundation/web-runtime/reactive-state', 'test/foundation/reactive-state/unit'):
            shutil.copytree(ROOT / relative, snapshot / relative,
                            ignore=shutil.ignore_patterns('Cargo.lock', 'target'))
        manifest = snapshot / 'src/foundation/web-runtime/reactive-state/Cargo.toml'
        source_package = ROOT / 'src/foundation/web-runtime/reactive-state'
        text = rebase_dependency_paths(manifest.read_text(), source_package)
        manifest.write_text(text + '\n[workspace]\n')
        cargo('generate-lockfile', '--offline', '--manifest-path', str(manifest))
        for features in ([], ['--features', 'effects']):
            result = cargo('test', '--locked', '--offline', '--lib', '--manifest-path', str(manifest),
                           '--target-dir', str(TARGET), *features, capture=True)
            output = result.stdout + result.stderr
            print(output, end='', flush=True)
            passed = sum(map(int, re.findall(r'test result: ok\. (\d+) passed', output)))
            if result.returncode or 'warning:' in output or passed < 14:
                raise AssertionError('retained reactive unit tests did not pass cleanly')


def check():
    check_browser_futures_features()
    check_reactive_units()
    TARGET.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='csr-scope-', dir=ROOT / 'target') as temporary:
        work = Path(temporary)
        (work / 'src').mkdir()
        manifest = work / 'Cargo.toml'
        manifest.write_text('[package]\nname="csr-template-contract"\nversion="0.0.0"\nedition="2021"\n'
                            '[workspace]\n[lib]\ncrate-type=["cdylib", "rlib"]\n[dependencies]\n' + '\n'.join([
            'view-renderer={path="' + str(ROOT / 'src/foundation/web-runtime/view-renderer') + '",features=["reactive-state","shared-value"]}',
            'dom-helpers={path="' + str(ROOT / 'src/foundation/web-runtime/dom-helpers') + '"}',
            'reactive-state={path="' + str(ROOT / 'src/foundation/web-runtime/reactive-state') + '",features=["effects"]}',
            'wasm-bindgen={path="' + str(ROOT / 'src/foundation/web-runtime/wasm-bindgen') + '"}',
            'js-sys={path="' + str(ROOT / 'src/foundation/web-runtime/js-sys') + '"}',
            'next_tuple={path="' + str(ROOT / 'src/foundation/collections/next_tuple') + '"}',
            'slotmap={path="' + str(ROOT / 'src/foundation/collections/slotmap') + '"}',
            'or_poisoned={path="' + str(ROOT / 'src/foundation/concurrency/or_poisoned') + '"}',
            'const_str_slice_concat={path="' + str(ROOT / 'src/foundation/text-unicode/const_str_slice_concat') + '"}',
            'throw_error={path="' + str(ROOT / 'src/foundation/diagnostics/throw_error') + '"}',
            'console_error_panic_hook={path="' + str(ROOT / 'src/foundation/diagnostics/console_error_panic_hook') + '"}',
            'gloo-events={path="' + str(ROOT / 'src/foundation/web-runtime/gloo-events') + '"}',
            'gloo-timers={path="' + str(ROOT / 'src/foundation/web-runtime/gloo-timers') + '"}',

            'web-sys={path="' + str(ROOT / 'src/foundation/web-runtime/web-sys') + '",features=["Window","Document","HtmlInputElement","Event"]}',
        ]) + '\n')
        source = work / 'src/lib.rs'
        source.write_text((ROOT / 'test/tools/web-bindings/fixtures/csr-template.rs').read_text() + '\nmod foundation_browser;\n')
        shutil.copyfile(ROOT / 'test/tools/web-bindings/fixtures/foundation-browser.rs', work / 'src/foundation_browser.rs')
        cargo('generate-lockfile', '--offline', '--manifest-path', str(manifest))
        command = ['check', '--locked', '--offline', '--manifest-path', str(manifest), '--target-dir', str(TARGET)]
        cargo(*command)
        cargo("test", "--locked", "--offline", "--lib", "--manifest-path", str(manifest), "--target-dir", str(TARGET))
        cargo('build', '--locked', '--offline', '--release', '--target', 'wasm32-unknown-unknown',
              '--manifest-path', str(manifest), '--target-dir', str(TARGET))
        output = TARGET / 'output'
        if output.exists():
            shutil.rmtree(output)
        wasm = TARGET / 'wasm32-unknown-unknown/release/csr_template_contract.wasm'
        arguments = [str(wasm), '--target', 'web', '--no-typescript', '--out-name', 'fixture', '--out-dir', str(output)]
        if tool := os.environ.get('ZXS_WEB_BINDGEN'):
            subprocess.run([tool, *arguments], cwd=ROOT, check=True)
        else:
            cargo('run', '--locked', '--offline', '-p', 'web-bindings', '--', *arguments)
        (output / 'index.html').write_text('''<!doctype html><meta charset="utf-8"><body>pending<script type="module">
try {
 const api=await import('./fixture.js'); await api.default();
 const foundation=JSON.parse(await api.run_foundation_probe());
 const template=JSON.parse(await api.run_template_probe());
 const errors=[]; const original=console.error; let trapped=false;
 console.error=(...args)=>{errors.push(args.map(String).join(' '));};
 try {api.run_foundation_panic_probe();} catch(error) {trapped=error instanceof WebAssembly.RuntimeError;}
 finally {console.error=original;}
 if(!trapped || errors.filter(value=>value.includes('foundation-console-panic-witness')).length!==1)
   throw new Error('panic hook did not produce exactly one message and a WASM trap');
 if(foundation.status!=='passed' || template.status!=='passed') throw new Error('probe failed');
 template.checks.push(...foundation.checks, 'console panic hook message and trap');
 document.body.textContent=JSON.stringify(template);
}
catch(error) { document.body.textContent=JSON.stringify({status:'failed',message:String(error),stack:error?.stack}); }
</script>''')
        # Compile exported tracing macros from an alias, without web-interface in scope.
        source.write_text((ROOT / 'test/ci/fixtures/tracing-props.rs').read_text())
        cargo(*command, '--features', 'dom-helpers/trace-component-props')
        cargo('test', '--locked', '--offline', '--lib', '--manifest-path', str(manifest),
              '--target-dir', str(TARGET), '--features', 'dom-helpers/trace-component-props')
        cargo(*command, '--target', 'wasm32-unknown-unknown', '--features', 'dom-helpers/trace-component-props')
        retired = {
            'owner_shared_context': 'pub fn old() { reactive_state::owner::Owner::current_shared_context(); }',
            'owner_hydration': 'pub fn old() { reactive_state::owner::Owner::with_hydration(|| ()); }',
            'owner_no_hydration': 'pub fn old() { reactive_state::owner::Owner::with_no_hydration(|| ()); }',
            'hydration_marker': 'use reactive_state::owner::IsHydrating;',
            'server_module': 'use view_renderer::ssr;',
            'nightly_static_views': 'use view_renderer::view::static_types;',
            'nightly_static_attributes': 'use view_renderer::html::attribute::StaticAttr;',
            'islands': 'use view_renderer::html::islands;',
            'serialized_keys': 'use view_renderer::view::keyed::SerializableKey;',
            'doctype': 'pub fn old() { view_renderer::html::doctype("html"); }',
            'server_hydration': 'pub fn old<T:view_renderer::view::RenderHtml>(value:T) { value.hydrate::<true>(); }',
            'attribute_output': 'pub fn old<T:view_renderer::html::attribute::Attribute>(value:T) { value.to_html(); }',
        }
        for method in ['to_html', 'to_html_with_buf', 'to_html_branching', 'to_html_stream_in_order',
                       'to_html_stream_out_of_order', 'to_html_async_with_buf', 'resolve', 'dry_resolve', 'hydrate_async']:
            retired[method] = f'pub fn old<T:view_renderer::view::RenderHtml>(mut value:T) {{ value.{method}(); }}'
        for name, code in {
            'typed_canvas': 'use view_renderer::html::element::canvas;',
            'typed_audio': 'use view_renderer::html::element::audio;',
            'typed_select': 'use view_renderer::html::element::select;',
            'typed_svg_animation': 'use view_renderer::svg::animate;',
            'typed_svg_use': 'use view_renderer::svg::Use;',
            'typed_mathml': 'use view_renderer::mathml;',
            'inline_event_attributes': 'use view_renderer::html::attribute::GlobalOnAttributes;',
            'typed_touch_event': 'use view_renderer::html::event::touchstart;',
            'typed_touch_type': 'use view_renderer::html::event::TouchEvent;',
            'unused_attribute': 'use view_renderer::html::attribute::accept;',
            'removed_web_api': 'use web_sys::WebGlRenderingContext;',
        }.items():
            retired[name] = code
        for name, code in retired.items():
            source.write_text(code)
            result = cargo(*command, capture=True)
            if not result.returncode or not any(code in result.stderr for code in ['error[E0432]', 'error[E0425]', 'error[E0599]', 'error[E0107]']):
                raise AssertionError(f'{name} accepted or failed for another reason: {result.stderr}')
        source.write_text('')
        retired_features = {
            'view-renderer': ['ssr', 'hydrate', 'islands', 'mark_branches', 'testing', 'sledgehammer', 'reactive_stores', 'error-hook', 'nightly'],
            'reactive-state': ['hydration', 'subsecond', 'nightly'],
            'dom-helpers': ['hydration'],
            'web-sys': ['AudioContext', 'WebGlRenderingContext', 'Gamepad', 'XrSession'],
        }
        feature_checks = sum(map(len, retired_features.values()))
        for package, features in retired_features.items():
            for feature in features:
                qualified = package + '/' + feature
                result = cargo(*command, '--features', qualified, capture=True)
                if not result.returncode or 'does not have that feature' not in result.stderr:
                    raise AssertionError(f'{qualified} accepted or failed for another reason: {result.stderr}')
        policy = json.loads((ROOT / 'tools/rust-sources/web-scope.json').read_text())
        declared = tomllib.loads((ROOT / 'src/foundation/web-runtime/web-sys/Cargo.toml').read_text())['features']
        if set(declared) != set(policy['web_sys_features']):
            raise AssertionError('web-sys feature policy drift')
        generated = {p.stem.removeprefix('gen_') for p in (ROOT / 'src/foundation/web-runtime/web-sys/src/features').glob('gen_*.rs')}
        if generated != set(declared) - {'default', 'std'}:
            raise AssertionError('web-sys source/feature closure drift')
        record = {
            'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'git_tree': subprocess.check_output(['git', 'rev-parse', 'HEAD^{tree}'], cwd=ROOT, text=True).strip(),
            'worktree_has_changes': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT)),
            'source_receipt_sha256': hashlib.sha256((ROOT / 'compliance/rust-sources.json').read_bytes()).hexdigest(),
            'web_scope_sha256': hashlib.sha256((ROOT / 'tools/rust-sources/web-scope.json').read_bytes()).hexdigest(),
            'tracing_macro_checks': ['native-check', 'native-test', 'wasm-check'],
            'tracing_macro_fixture_sha256': hashlib.sha256((ROOT / 'test/ci/fixtures/tracing-props.rs').read_bytes()).hexdigest(),
            'retired_api_checks': len(retired),
            'retired_feature_checks': feature_checks,
            'cargo_lock_sha256': hashlib.sha256((ROOT / 'Cargo.lock').read_bytes()).hexdigest(),
            'foundation_fixture_sha256': hashlib.sha256((ROOT / 'test/tools/web-bindings/fixtures/foundation-browser.rs').read_bytes()).hexdigest(),
            'fixture_source_sha256': hashlib.sha256((ROOT / 'test/tools/web-bindings/fixtures/csr-template.rs').read_bytes()).hexdigest(),
            'outputs_sha256': {name: hashlib.sha256((output / name).read_bytes()).hexdigest()
                               for name in ['fixture.js', 'fixture_bg.wasm']},
        }
        (output / 'validation.json').write_text(json.dumps(record, indent=2) + '\n')
        print(f'CSR contract: browser template compiled; {len(retired)} retired API and {feature_checks} feature checks passed')


if __name__ == '__main__':
    check()
