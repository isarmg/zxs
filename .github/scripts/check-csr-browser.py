#!/usr/bin/env python3
"""Exercise the generated CSR template fixture in a real sandboxed browser."""
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import json
import os
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / 'target/csr-template-check/output'

FOUNDATION_CHECKS = {
    'next_tuple values and ownership',
    'or_poisoned real lock guards',
    'slotmap stale key generation',
    'const concatenation and class separators',
    'throw_error nested hook restoration',
    'js-sys values and identity',
    'web-sys DOM text and event target',
    'gloo-events once and drop cleanup',
    'gloo-timers fire and cancel cleanup',
    'console panic hook message and trap',
}


def validate_result(result):
    if result.get('status') != 'passed':
        raise AssertionError(result)
    missing = FOUNDATION_CHECKS - set(result['checks'])
    if missing:
        raise AssertionError(f'missing foundation browser contracts: {sorted(missing)}')


def check():
    for name in ['index.html', 'fixture.js', 'fixture_bg.wasm', 'validation.json']:
        if not (FIXTURE / name).is_file():
            raise RuntimeError(f'missing CSR fixture artifact: {name}')

    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(FIXTURE), **kwargs)

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with socket.socket() as reserved:
        reserved.bind(('127.0.0.1', 0))
        port = reserved.getsockname()[1]
    session = None
    log = tempfile.TemporaryFile()
    driver = subprocess.Popen([os.environ.get('CHROMEDRIVER', 'chromedriver'), f'--port={port}'],
                              stdout=log, stderr=log)

    def request(path, body=None, method=None):
        call = urllib.request.Request(f'http://127.0.0.1:{port}{path}',
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={'Content-Type': 'application/json'}, method=method)
        try:
            with urllib.request.urlopen(call, timeout=30) as response:
                return json.load(response)['value']
        except urllib.error.HTTPError as error:
            raise RuntimeError(error.read().decode()) from error

    try:
        for _ in range(100):
            if driver.poll() is not None:
                raise RuntimeError('ChromeDriver exited before readiness')
            try:
                request('/status')
                break
            except OSError:
                time.sleep(.1)
        else:
            raise RuntimeError('ChromeDriver did not become ready')
        options = {'args': ['--headless=new', '--disable-dev-shm-usage']}
        if binary := os.environ.get('CHROME_BIN'):
            options['binary'] = binary
        value = request('/session', {'capabilities': {'alwaysMatch': {
            'browserName': 'chrome', 'goog:chromeOptions': options, 'goog:loggingPrefs': {'browser': 'ALL'},
        }}})
        session = value['sessionId']
        prefix = '/session/' + session
        request(prefix + '/url', {'url': f'http://127.0.0.1:{server.server_port}/index.html'})
        for _ in range(200):
            content = request(prefix + '/execute/sync', {'script': 'return document.body.textContent', 'args': []})
            if content.startswith('{'):
                break
            time.sleep(.1)
        else:
            raise RuntimeError('CSR browser probe did not settle')
        result = json.loads(content)
        validate_result(result)
        result.update(browser=value['capabilities']['browserVersion'], sandbox_disabled=False,
                      validation=json.loads((FIXTURE / 'validation.json').read_text()))
        (FIXTURE / 'browser-result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
        print('PASS real browser template: ' + ', '.join(result['checks']))
    except Exception:
        if session:
            try:
                print(json.dumps(request(prefix + "/log", {"type": "browser"}), ensure_ascii=False)[-4000:])
            except OSError:
                pass
        log.seek(0)
        print(log.read().decode(errors='replace')[-4000:])
        raise
    finally:
        if session:
            try:
                request('/session/' + session, method='DELETE')
            except OSError:
                pass
        driver.terminate()
        driver.wait(timeout=15)
        server.shutdown()
        server.server_close()
        log.close()


if __name__ == '__main__':
    check()
