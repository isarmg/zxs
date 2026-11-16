#!/usr/bin/env python3
"""Run the bounded real-SDK descriptor regression using current Cargo evidence."""
import importlib.util
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GROUP = 'smb-vfs-manual-fd'
EXACT = [
    'module::tests::manual_fd_tests::'
    'manual_open_reads_its_real_fd_after_write_truncate_and_extend_without_workspace_identity',
    'module::tests::'
    'sync_and_async_immutable_reads_use_verified_rpc_bytes_with_a_directory_placeholder',
    'module::tests::mtime_tests::'
    'mtime_registered_sdk_callbacks_project_save_time_and_keep_read_handles_fixed',
    'module::tests::seek_tests::'
    'registered_readonly_seek_rejects_placeholder_geometry_and_rechecks_admission',
    'module::tests::seek_tests::'
    'registered_writable_seek_preserves_work_fd_position_length_and_kernel_errors',
    'module::tests::revoked_close_detaches_without_publishing_and_releases_descriptor',
    'module::tests::'
    'context_cleanup_closes_owned_records_without_publishing_and_snapshot_outlives_it',
]
TARGET = {
    'package': 'smb-vfs',
    'manifest': 'src/smb-vfs/Cargo.toml',
    'kind': 'lib',
    'default_features': True,
    'features': ['smb-sdk'],
    'exact': EXACT,
    'max_tests': len(EXACT),
    'timeout_seconds': 120,
    'selection_groups': [GROUP],
}


def main():
    # The existing SDK script validates the pinned Debian source and config.
    # A plain Ubuntu cargo test is not a substitute for this FFI boundary.
    for name in ('ZXS_SMB_SDK_SOURCE', 'ZXS_SMB_SDK_BUILD'):
        if not os.environ.get(name):
            raise ValueError(f'missing actual configured SDK: {name}')
    specification = importlib.util.spec_from_file_location(
        'targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
    ci = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(ci)
    run = ci.Run(ROOT)
    run.summary['scope'] = GROUP
    run.summary['unexecuted'] = (
        'Only the seven exact VFS descriptor tests. No Server cold recovery, '
        'system Samba/client acceptance, background synthesis or incremental format claim.')
    try:
        run.summary['head'] = run.command(['git', 'rev-parse', 'HEAD']).strip()
        lock = ci.digest(ROOT / 'Cargo.lock')
        run.summary['lockfile_sha256'] = lock
        run.summary['sdk'] = {
            'target': json.loads((ROOT / '.github/samba-debian-target.json').read_text()),
            'configured_header_sha256': ci.digest(
                Path(os.environ['ZXS_SMB_SDK_BUILD']) / 'include/config.h'),
            'source_changelog_sha256': ci.digest(
                Path(os.environ['ZXS_SMB_SDK_SOURCE']) / 'debian/changelog'),
            'recipe_sha256': ci.digest(ROOT / '.github/scripts/smb-sdk.sh'),
        }
        run.rust(TARGET)
        if any(run.summary[key] for key in ('failed', 'skipped', 'blocked')):
            raise RuntimeError('real SDK descriptor target failed; see exact logs')
        if sorted(item['name'] for item in run.summary['passed']) != sorted(EXACT):
            raise ValueError('exact descriptor tests were not all executed once')
        if ci.digest(ROOT / 'Cargo.lock') != lock:
            raise ValueError('Cargo.lock changed during descriptor verification')
        run.summary['status'] = 'selected-sdk-tests-passed'
    except Exception as error:
        run.summary.update(status='failed', error=f'{type(error).__name__}: {error}')
        raise
    finally:
        run.save()


if __name__ == '__main__':
    main()
