#!/usr/bin/env python3
"""Run phase-one gates from current Cargo JSON artifacts, independently of workspace tests."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
SERVER = 'storage_server'
FILE_TARGET = 'phase1_save_tags'
SMB_TARGET = 'phase1_smb_rpc'
DIRECTORY_TARGET = 'directory_git_index'
TAG_TARGET = 'phase1_tag_regressions'
COLD_TAG_TARGET = 'phase1_tag_cold_replay'
CONTRACT_TARGET = 'contract-check'
ADMISSION = 'backend_admission_tests::phase_one_write_admission::'
RETAINED = 'backend_admission_tests::retained_management_slots::'
SMB = 'smb_rpc::tests::default_share_tests::phase1_new_file_tests::'
SERVER_TESTS = (
    RETAINED + 'two_dropped_http_waits_keep_management_slots_until_backend_completion',
    RETAINED + 'two_timed_out_http_waits_keep_management_slots_until_backend_completion',
    RETAINED + 'tcp::two_disconnected_tcp_clients_keep_management_slots_until_workers_finish',
    RETAINED + 'tcp::two_timed_out_tcp_clients_keep_management_slots_until_workers_finish',
    *(ADMISSION + name for name in (
        'file_tags_revalidate_session_after_streamed_json',
        'disk_write_session_revalidates_session_after_streamed_json',
        'workspace_commit_revalidates_session_after_streamed_json',
        'canceled_tag_body_never_dispatches_and_allows_retry',
        'canceled_write_session_body_never_dispatches_and_allows_retry',
        'canceled_commit_body_never_dispatches_and_allows_retry',
        'interrupted_or_oversized_tag_body_does_not_dispatch',
        'interrupted_or_oversized_write_session_body_does_not_dispatch',
        'interrupted_or_oversized_commit_body_does_not_dispatch',
    )),
    SMB + 'phase1_primary_source_read_to_new_target_keeps_independent_history',
    SMB + 'phase1_primary_empty_create_publishes_once_on_close_or_flush_then_close',
    SMB + 'phase1_primary_exclusive_create_conflicts_preserve_saved_file_and_connection',
    'backend_storage_metadata::tests::disk_directory_names_preserve_order_and_check_backup_roots',
    'setup_recovery::activation::tests::unbrowseable_disk_names_are_rejected_before_activation_intent_or_writes',
    'management::storage_addition::tests::addition_rejects_unbrowseable_disk_names_without_persisting_a_plan',
    'management::storage_addition::tests::addition_rejects_colliding_new_primary_and_backup_roots',
    'setup_recovery::activation::tests::primary_import_recovery::single_primary_initialization_and_import_recover_without_replacing_identities',
    'setup_recovery::activation::tests::primary_import_recovery::missing_import_requests_preserve_published_data_without_fabricating_completion',
    'setup_recovery::activation::tests::first_single_disk_is_not_a_zero_member_pool',
    'setup_recovery::activation::tests::nonempty_selected_directory_is_rejected_before_intent_and_preserved',
    'setup_recovery::activation::tests::complete_genesis_without_ready_receipt_is_checked_and_completed_not_reinitialized',
    'setup_recovery::activation::tests::competing_first_activations_publish_one_identity_and_preserve_the_losing_root',
    'setup_recovery::activation::tests::missing_config_after_activation_never_looks_unconfigured',
    'setup_recovery::activation::tests::revoked_authority_after_storage_creation_never_completes_activation',
    'setup_recovery::activation::tests::durable_control_reads_reject_nonprivate_permissions_on_held_file',
)
FILE_TESTS = (
    'unicode_save_tags_unchanged_body_and_replacement_survive_cold_open',
    'empty_file_and_truncate_to_empty_keep_identity_and_node_labels',
    'same_named_files_on_two_level_one_disks_keep_independent_tags',
    'rejection::revoked_file_mutations_leave_history_and_bytes_unchanged_after_cold_open',
    'rejection::stale_tag_failure_is_atomic_and_does_not_consume_retry_identity',
    'directory_git::reviewed_index_and_later_work_survive_http_commit_replay_and_cold_open',
    'directory_git::dirty_work_blocks_branch_changes_until_explicit_checkpoint_and_checkout',
    'directory_git::child_reference_staging_keeps_parent_work_out_of_commit_across_cold_open',
    'discovery::cookie_discovery_inspection_and_activation_produce_browsable_primary_roots',
    'discovery::cookie_activation_rejects_missing_csrf_and_cross_origin_without_writes',
    'discovery::cookie_activation_rejects_unbrowseable_names_before_intent_and_can_retry',
    'discovery::replaced_directory_and_logged_out_cookie_cannot_activate_reviewed_selection',
)
DIRECTORY_TESTS = (
    'explicit_commit_freezes_staged_contents_and_preserves_later_work_across_restart',
    'unstaged_work_cannot_be_committed_or_discarded_by_branch_operations',
    'restaging_a_renamed_child_mount_commits_its_path_without_advancing_its_pin',
    'staging_a_child_reference_does_not_stage_the_parents_working_edits',
    'moving_the_repository_root_does_not_change_its_relative_child_pins',
    'staged_file_is_committed_without_resurrecting_its_later_working_deletion',
    'staged_deletion_does_not_delete_a_recreated_working_file',
    'deleted_child_boundary_is_not_reused_by_a_same_path_replacement',
    'inserting_an_intermediate_repository_drops_only_the_parents_direct_pin',
    'moving_a_staged_file_changes_the_working_location_not_the_reviewed_snapshot',
)
TAG_TESTS = (
    'fresh_duplicate_input_is_one_tag_but_exact_receipt_still_binds_raw_request',
    'reused_old_path_does_not_retarget_file_tags_or_break_exact_replay',
    'current_tags_ignore_retired_directory_identity_and_keep_root_and_history',
)
SMB_TESTS = (
    'public_smb_bridge_read_to_new_file_keeps_independent_history_after_restart',
    'public_smb_bridge_empty_create_close_and_flush_publish_once',
    'public_smb_bridge_exclusive_conflicts_preserve_content_and_connection',
)
COLD_TAG_TESTS = (
    'old_save_and_tag_receipts_do_not_rewind_newer_content_after_cold_open',
    'noop_tag_receipts_remain_permanent_after_another_save_and_two_cold_opens',
)
CONTRACT_TESTS = ('tests::file_merge_tags_and_scope_backups_require_explicit_reviewed_baselines',)

# Manifest and source identity prevent selecting a namesake artifact from another package.
TARGETS = {
    SERVER: ('src/server/Cargo.toml', 'src/server/src/lib.rs', ['lib'], {'default', 'core'}),
    FILE_TARGET: ('src/server/Cargo.toml', 'test/server/integration/phase1_save_tags.rs', ['test'], {'default', 'core'}),
    SMB_TARGET: ('src/server/Cargo.toml', 'test/server/integration/phase1_smb_rpc.rs', ['test'], {'default', 'core'}),
    DIRECTORY_TARGET: ('src/engine/Cargo.toml', 'test/engine/integration/directory_git_index.rs', ['test'], set()),
    TAG_TARGET: ('src/engine/Cargo.toml', 'test/engine/integration/phase1_tag_regressions.rs', ['test'], set()),
    COLD_TAG_TARGET: ('src/engine/Cargo.toml', 'test/engine/integration/phase1_tag_cold_replay.rs', ['test'], set()),
    CONTRACT_TARGET: ('tools/contract-check/Cargo.toml', 'tools/contract-check/src/main.rs', ['bin'], set()),
}


def digest(path):
    result = hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            result.update(block)
    return result.hexdigest()


def test_artifacts(output, root, target_dir, expected):
    """Only successful artifacts emitted by this invocation may become executables."""
    root, target_dir = Path(root).resolve(), Path(target_dir).resolve()
    found = {name: [] for name in expected}
    finished = []
    for line in output.splitlines():
        item = json.loads(line)
        if item.get('reason') == 'compiler-message':
            diagnostic = item['message']
            if diagnostic['level'] in {'warning', 'error'}:
                raise ValueError(diagnostic.get('rendered') or diagnostic['message'])
        if item.get('reason') == 'build-finished':
            finished.append(item['success'] is True)
        if item.get('reason') != 'compiler-artifact':
            continue
        name = item['target']['name']
        if name not in found or item['profile'].get('test') is not True:
            continue
        manifest, source, kind, required = TARGETS[name]
        if item['target']['kind'] != kind:
            continue
        if (Path(item['manifest_path']).resolve() != root / manifest
                or Path(item['target']['src_path']).resolve() != root / source):
            raise ValueError(f'{name}: artifact belongs to another source or package')
        features = item['features']
        if not isinstance(features, list) or not all(isinstance(value, str) for value in features):
            raise ValueError(f'{name}: invalid Cargo feature evidence')
        if not required <= set(features):
            raise ValueError(f'{name}: current artifact lacks required features {sorted(required)}')
        executable = item.get('executable')
        if not isinstance(executable, str) or not Path(executable).is_absolute():
            raise ValueError(f'{name}: missing current test executable')
        executable = Path(executable).resolve()
        if not executable.is_relative_to(target_dir) or not executable.is_file() or not os.access(executable, os.X_OK):
            raise ValueError(f'{name}: executable is absent or outside this gate target directory')
        found[name].append({'path': str(executable), 'sha256': digest(executable),
                            'features': features, 'package_id': item['package_id'],
                            'manifest': manifest, 'source': source})
    if finished != [True] or any(len(values) != 1 for values in found.values()):
        raise ValueError(f'expected one successful current Cargo artifact per target: {found}')
    return {name: values[0] for name, values in found.items()}


def listed_tests(output, expected, whole_target=False):
    names = []
    for line in output.splitlines():
        if line.endswith(': test'):
            names.append(line[:-6])
        elif line and not re.fullmatch(r'\d+ tests?, \d+ benchmarks?', line):
            raise ValueError(f'unexpected test listing: {line}')
    if not names or len(names) != len(set(names)) or not expected or len(expected) != len(set(expected)):
        raise ValueError('empty or duplicate test inventory')
    actual, wanted = set(names), set(expected)
    if not wanted <= actual or (whole_target and actual != wanted):
        raise ValueError(f'test inventory mismatch: missing={sorted(wanted - actual)}, extra={sorted(actual - wanted)}')
    return names


def passed_tests(output, count, whole_target=False):
    summaries = re.findall(
        r'^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;.*$',
        output, re.MULTILINE)
    if count <= 0 or len(summaries) != 1:
        raise ValueError('missing one nonzero test completion summary')
    passed, failed, ignored, measured, filtered = map(int, summaries[0])
    if (passed != count or failed or ignored or measured or (whole_target and filtered)):
        raise ValueError(f'ineffective test execution: {summaries[0]}, expected {count}')


def build_commands(target_dir):
    common = ['cargo', 'test', '--locked', '--offline', '--no-run', '--message-format=json',
              '--target-dir', str(target_dir)]
    return (
        ('build-server', common + ['-p', 'storage-server', '--features', 'core', '--lib',
                                  '--test', FILE_TARGET, '--test', SMB_TARGET], (SERVER, FILE_TARGET, SMB_TARGET)),
        ('build-engine-integrations', common + ['-p', 'storage-engine', '--test', DIRECTORY_TARGET,
                                               '--test', TAG_TARGET, '--test', COLD_TAG_TARGET],
         (DIRECTORY_TARGET, TAG_TARGET, COLD_TAG_TARGET)),
        ('build-contract', common + ['-p', 'contract-check', '--bin', CONTRACT_TARGET], (CONTRACT_TARGET,)),
    )


class Gate:
    def __init__(self, root):
        self.root = Path(root).resolve()
        self.evidence = self.root / 'target/phase-one-core'
        self.logs = self.evidence / 'logs'
        self.logs.mkdir(parents=True, exist_ok=True)
        self.target = self.evidence / 'build'
        fixtures = self.evidence / 'fixtures'
        fixtures.mkdir(mode=0o700, exist_ok=True)
        # Disk identity admission rejects tmpfs. Do not inherit runner TMPDIR.
        self.environment = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_BUILD_JOBS='1',
                                CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0',
                                TMPDIR=str(fixtures))
        self.summary = {'status': 'incomplete', 'commands': [], 'artifacts': {}, 'passed': []}
        self.save()

    def save(self):
        (self.evidence / 'summary.json').write_text(json.dumps(self.summary, indent=2) + '\n')

    def run(self, label, command, timeout):
        record = {'label': label, 'command': command, 'cwd': str(self.root), 'timeout_seconds': timeout}
        self.summary['commands'].append(record)
        self.save()
        print('+ ' + ' '.join(command), flush=True)
        start = time.monotonic()
        try:
            with (self.logs / (label + '.stdout.log')).open('w') as stdout, (self.logs / (label + '.stderr.log')).open('w') as stderr:
                process = subprocess.Popen(command, cwd=self.root, env=self.environment,
                                           stdout=stdout, stderr=stderr, start_new_session=True)
                try:
                    returncode = process.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    record['timed_out'] = True
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass  # The process group exited at the timeout boundary.
                    process.wait()
                    raise
            record['returncode'] = returncode
            if returncode:
                for suffix in ('stdout', 'stderr'):
                    print((self.logs / (label + '.' + suffix + '.log')).read_text()[-16000:], file=sys.stderr)
                raise subprocess.CalledProcessError(returncode, command)
            return (self.logs / (label + '.stdout.log')).read_text()
        finally:
            record['seconds'] = round(time.monotonic() - start, 3)
            self.save()

    def execute(self):
        try:
            manifest = tomllib.loads((self.root / 'Cargo.toml').read_text())
            if 'workspace' not in manifest or not (self.root / 'Cargo.lock').is_file():
                raise ValueError('phase-one gate requires the root workspace and its existing lockfile')
            lock_hash = digest(self.root / 'Cargo.lock')
            self.summary['lockfile_sha256'] = lock_hash
            self.summary['head'] = self.run('source-head', ['git', 'rev-parse', 'HEAD'], 30).strip()
            artifacts = {}
            for label, command, expected in build_commands(self.target):
                output = self.run(label, command, 1800)
                artifacts.update(test_artifacts(output, self.root, self.target, expected))
                if digest(self.root / 'Cargo.lock') != lock_hash:
                    raise ValueError('Cargo changed the root lockfile')
                self.summary['artifacts'] = artifacts
                self.save()
            # Enumerate all required targets before executing any test. No glob,
            # old receipt or successful zero-test run can satisfy this inventory.
            suites = ((SERVER, SERVER_TESTS, False), (FILE_TARGET, FILE_TESTS, True),
                      (SMB_TARGET, SMB_TESTS, True),
                      (DIRECTORY_TARGET, DIRECTORY_TESTS, True), (TAG_TARGET, TAG_TESTS, True),
                      (COLD_TAG_TARGET, COLD_TAG_TESTS, True),
                      (CONTRACT_TARGET, CONTRACT_TESTS, False))
            for target, expected, whole in suites:
                binary = artifacts[target]['path']
                output = self.run('list-' + target, [binary, '--list', '--format', 'terse'], 60)
                names = listed_tests(output, expected, whole)
                artifacts[target]['listed_tests'] = names
                self.save()
            for target, expected, whole in suites:
                binary = artifacts[target]['path']
                if digest(binary) != artifacts[target]['sha256']:
                    raise ValueError(f'{target}: executable changed after the current Cargo build')
                groups = (tuple(expected),) if whole else tuple((name,) for name in expected)
                for index, names in enumerate(groups):
                    command = [binary, '--nocapture', '--test-threads=1']
                    if not whole:
                        command += ['--exact', names[0]]
                    output = self.run(f'run-{target}-{index:02}', command, 300)
                    passed_tests(output, len(names), whole)
                    self.summary['passed'].extend({'target': target, 'name': name} for name in names)
                    self.save()
            self.summary['status'] = 'passed'
            self.summary['passed_count'] = len(self.summary['passed'])
        except Exception as error:
            self.summary['status'] = 'failed'
            self.summary['error'] = f'{type(error).__name__}: {error}'
            raise
        finally:
            self.save()


if __name__ == '__main__':
    Gate(ROOT).execute()
