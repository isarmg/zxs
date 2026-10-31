"""Conservative serialized-map lower bound; not a replacement for Engine admission."""
if not __debug__:
    raise RuntimeError('Budget contract proof requires Python assertions')
import json

FILES = 500
SEED_SEQUENCE = 220
ORIGINAL_SAVES = 20
TAIL_ROUNDS = 40
PRINCIPAL_LIMIT = 64 * 1024 * 1024


def proof():
    # All original paths have the same ASCII width. The production reservation
    # normalizes these three IDs to >=64 bytes and generation to u64::MAX.
    context = dict(node_id='x'*64, selection_generation=str(2**64-1),
                   branch_id='x'*64, inherited_from='x'*64)
    encode = lambda value: json.dumps(value, sort_keys=True, separators=(',', ':'))
    key = 'repo_'+'0'*32+'/dir-000/file-000000.txt'
    context_row = len(encode(key)) + 1 + len(encode(context)) + 1
    # Generated IDs contain a 32-hex UUID plus a prefix. Ignoring the prefix
    # understates both selected_file_branches keys and values.
    branch_row = 2+32 + 1 + 2+32 + 1
    two_maps = (2 + FILES*context_row-1) + (2 + FILES*branch_row-1)
    # Exclude repository creation and the first upload workspace entirely.
    # Every later save captures at least the unchanged 500 current file rows.
    initial_full = SEED_SEQUENCE-2 + ORIGINAL_SAVES
    tail_full = 3*TAIL_ROUNDS
    required = (initial_full+tail_full)*two_maps
    assert required > PRINCIPAL_LIMIT
    return dict(schema='original-220-budget-contract-v1', files=FILES,
                seed_sequence=SEED_SEQUENCE, original_saves=ORIGINAL_SAVES,
                original_end_sequence=SEED_SEQUENCE+ORIGINAL_SAVES,
                tail_rounds=TAIL_ROUNDS, tail_publications=tail_full,
                required_end_sequence=SEED_SEQUENCE+ORIGINAL_SAVES+tail_full,
                two_map_min_bytes_per_workspace=two_maps,
                original_full_workspace_min=initial_full,
                original_two_map_min_bytes=initial_full*two_maps,
                tail_full_workspace_min=tail_full,
                tail_two_map_min_bytes=tail_full*two_maps,
                required_two_map_min_bytes=required,
                principal_limit_bytes=PRINCIPAL_LIMIT,
                excess_min_bytes=required-PRINCIPAL_LIMIT,
                supported_full_workspaces_upper=PRINCIPAL_LIMIT//two_maps,
                support='FAIL_DEFAULT_BUDGET',
                exclusions=['initial upload workspace', 'workspace headers', 'staged-file records',
                            'selection generation maps', 'newly added file contexts', 'ID prefixes'],
                consequence='continuous 240-to-360 contract cannot fit; sharded samples cannot erase this failure')


if __name__ == '__main__':
    print(json.dumps(proof(), sort_keys=True))
