#!/usr/bin/env python3
"""Check erased-reference borrow lifetimes and checked renderer downcasts."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

from pathlib import Path
import json
import re
import runpy
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def run(arguments, environment):
    result = subprocess.run(['cargo', *arguments], cwd=ROOT, env=environment,
                            capture_output=True, text=True)
    output = result.stdout + result.stderr
    print(output, end='', flush=True)
    assert result.returncode == 0, arguments
    return output


def check():
    environment = runpy.run_path(str(ROOT / '.github/scripts/backend_environment.py'))['matrix_environment']()
    environment.setdefault('CARGO_TARGET_DIR', str(ROOT / 'target/erased-boundaries'))
    output = run(['test', '--locked', '--offline', '-p', 'erased', '--doc'], environment)
    compile_fail = re.findall(r'^test .*erased_mut_ref\.rs.* - compile fail \.\.\. ok$', output, re.M)
    assert len(compile_fail) == 2, 'both erased-reference aliasing examples must be rejected'

    with tempfile.TemporaryDirectory(prefix='erased-renderer-boundaries-') as temporary:
        work = Path(temporary)
        (work / 'src').mkdir()
        (work / 'src/lib.rs').write_text('')
        (work / 'Cargo.toml').write_text(
            '[package]\nname="erased-boundary-consumer"\nversion="0.0.0"\nedition="2021"\n'
            '[workspace]\n[dependencies]\nview-renderer={path='
            + json.dumps(str(ROOT / 'src/foundation/web-runtime/view-renderer')) + '}\n'
            '[[test]]\nname="downcasts"\npath='
            + json.dumps(str(ROOT / 'test/ci/fixtures/view-renderer-erased.rs')) + '\n')
        manifest = str(work / 'Cargo.toml')
        run(['generate-lockfile', '--offline', '--manifest-path', manifest], environment)
        for flags in ('', '--cfg erase_components'):
            selected = dict(environment, RUSTFLAGS=flags)
            output = run(['test', '--locked', '--offline', '--manifest-path', manifest,
                          '--test', 'downcasts'], selected)
            passed = re.findall(r'^test erased_safety_regressions::\S+(?: - should panic)? \.\.\. ok$', output, re.M)
            assert len(passed) == 10, 'both erased containers need all ten runtime regressions'
    print('Erased boundaries verified: two compile-fail borrow examples and ten checked-downcast tests in both component modes.')


if __name__ == '__main__':
    check()
