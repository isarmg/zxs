#!/usr/bin/env python3
"""Map every active foundation package to its consumers, targets, features and test execution.

Inputs, all read from the repository:

* ``compliance/rust-sources.json``: active packages, the registered entry
  profiles with target, role and resolved features, retired/superseded records;
* ``cargo tree`` per profile manifest: the first-party consumers that reach a
  package through normal or build edges and the features that consumer resolves;
* ``compliance/external-tests/*.json``: the external test roots of each owner;
* ``tools/rust-sources/foundation-test-gates.json``: reviewed gate entries;
* ``.github/scripts/check-foundation-behaviour.py``: its suite table.

``--write`` regenerates the JSON map and the Markdown report; ``--check``
requires both to equal a fresh generation. Classification is computed here,
never copied from a report.
"""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import argparse
from collections import defaultdict
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / 'compliance/rust-sources.json'
WORKSPACES = ROOT / 'tools/rust-sources/workspaces.json'
GATES = ROOT / 'tools/rust-sources/foundation-test-gates.json'
EXTERNAL = ROOT / 'compliance/external-tests'
BEHAVIOUR = ROOT / '.github/scripts/check-foundation-behaviour.py'
JSON_OUTPUT = ROOT / 'test/ci/fixtures/foundation-test-coverage.json'
MARKDOWN_OUTPUT = ROOT / 'docs/validation/foundation-test-coverage-map.md'
KINDS = {'U': 'retained upstream tests', 'P': 'project-authored tests or fixtures',
         'F': 'companion macro exercised by an executed suite', 'I': 'compiled or linked only'}
EXECUTING = {'U', 'P', 'F'}
HIGH_RISK_GROUPS = {'concurrency', 'platform', 'networking', 'cryptography'}
TEST_ATTRIBUTE = re.compile(r'^\s*#\[(?:[a-z_]+::)?test\]|^\s*#\[wasm_bindgen_test\]', re.M)
TREE_LINE = re.compile(r'^(\S+) v\S+(?: \(proc-macro\))? \(([^)]+)\)\|(\S*)(?: \(\*\))?$')


def load_behaviour():
    spec = importlib.util.spec_from_file_location('foundation_behaviour', BEHAVIOUR)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def package_name(directory):
    return tomllib.loads((ROOT / directory / 'Cargo.toml').read_text())['package']['name']


def consumers(profiles):
    """First-party consumers grouped by profile, target and the features each consumer resolves."""
    found = defaultdict(lambda: defaultdict(set))
    for profile in profiles:
        for manifest in profile['manifests']:
            member = package_name(Path(manifest).parent)
            for target in profile['targets']:
                command = ['cargo', 'tree', '--locked', '--offline', '--color', 'never',
                           '-p', member, '--target', target,
                           '-e', 'normal,build', '--prefix', 'none', '-f', '{p}|{f}']
                for feature in profile.get('features', []):
                    command += ['--features', feature]
                output = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=True).stdout
                for line in output.splitlines():
                    match = TREE_LINE.match(line)
                    assert match, f'unexpected cargo tree line: {line!r}'
                    name, path, features = match.groups()
                    relative = Path(path).resolve().relative_to(ROOT).as_posix()
                    if not relative.startswith('src/foundation/') or name == member:
                        continue
                    key = (profile['id'], target, tuple(sorted(filter(None, features.split(',')))))
                    found[relative][key].add(member)
    return {relative: [{'profile': profile, 'target': target, 'features': list(features),
                        'consumers': sorted(members)}
                       for (profile, target, features), members in sorted(groups.items())]
            for relative, groups in found.items()}


def test_material():
    roots = defaultdict(list)
    for manifest in sorted(EXTERNAL.glob('*.json')):
        for owner in json.loads(manifest.read_text())['owners']:
            roots[owner['directory']].extend(owner['roots'])
    material = {}
    for directory, paths in roots.items():
        files = [path for root in paths for path in sorted((ROOT / root).rglob('*')) if path.is_file()]
        attributes = sum(len(TEST_ATTRIBUTE.findall(path.read_text(errors='replace')))
                         for path in files if path.suffix == '.rs')
        material[directory] = {'roots': sorted(paths), 'files': len(files), 'test_attributes': attributes}
    return material


def behaviour_entries(behaviour):
    entries = defaultdict(list)
    script = BEHAVIOUR.relative_to(ROOT).as_posix()
    for name in behaviour.UNIT_SUITES:
        entries[name].append({'kind': 'U', 'shard': 'foundation-async', 'scripts': [script],
                              'detail': 'cargo test --lib -p ' + name + '; names equal the generated inventory'})
    for suite in behaviour.INTEGRATION_SUITES:
        retained = [path for path in suite.tests if path.startswith('test/foundation/')]
        authored = [path for path in suite.tests if not path.startswith('test/foundation/')]
        if retained:
            entries[suite.subject].append({'kind': 'U', 'shard': 'foundation-async', 'scripts': [script],
                                           'detail': 'retained ' + ', '.join(retained)
                                           + '; names equal the generated inventory'})
        if authored:
            entries[suite.subject].append({'kind': 'P', 'shard': 'foundation-async', 'scripts': [script],
                                           'detail': ', '.join(authored) + '; names equal the generated inventory'})
    return entries


def invoked_scripts():
    """Scripts reachable from CI entry points; a registry entry must name one of them."""
    text = '\n'.join(path.read_text() for path in sorted((ROOT / '.github').rglob('*'))
                     if path.is_file() and path.suffix in ('.sh', '.py', '.yml'))
    return text


def generate():
    catalog = json.loads(CATALOG.read_text())
    workspaces = json.loads(WORKSPACES.read_text())
    production = {profile['id'] for profile in workspaces['profiles'] if profile['production']}
    active = sorted(directory for directory in catalog['packages'] if directory.startswith('src/foundation/'))
    names = {directory: package_name(directory) for directory in active}
    by_name = {name: directory for directory, name in names.items()}
    assert len(by_name) == len(active), 'active foundation package names are not unique'

    registry = json.loads(GATES.read_text())
    assert registry['schema'] == 1
    ci = invoked_scripts()
    gates = defaultdict(list)
    for entry in registry['entries']:
        assert entry['package'] in by_name, f'gate entry for an inactive package: {entry["package"]}'
        assert entry['kind'] in KINDS, entry
        for script in entry['scripts']:
            assert (ROOT / script).is_file(), script
            assert Path(script).name in ci, f'{script} is not invoked by any CI entry point'
        gates[entry['package']].append({key: entry[key] for key in ('kind', 'shard', 'scripts', 'detail')
                                        if key in entry})
    behaviour = load_behaviour()
    for name, entries in behaviour_entries(behaviour).items():
        assert name in by_name, name
        gates[name].extend(entries)

    profiles = defaultdict(dict)
    for resolution in catalog['resolutions']:
        for directory, package in resolution['packages'].items():
            if directory in names:
                profiles[directory][resolution['profile']] = {
                    'target': resolution['target'], 'roles': package['roles'], 'features': package['features']}
    reached = consumers(workspaces['profiles'])
    material = test_material()

    packages = []
    for directory in active:
        name = names[directory]
        reached_profiles = profiles[directory]
        assert reached_profiles, f'{directory} is outside every registered closure'
        in_production = bool(set(reached_profiles) & production)
        runtime = any('runtime' in reached_profiles[profile]['roles'] for profile in reached_profiles
                      if profile in production)
        executing = sorted({gate['kind'] for gate in gates[name]} & EXECUTING)
        if executing:
            classification, risk = 'executed', None
        elif in_production:
            classification = 'needed-not-executed'
            group = directory.split('/')[2]
            risk = 'high' if runtime and group in HIGH_RISK_GROUPS else 'medium' if runtime else 'low'
        else:
            classification, risk = 'reference-only', None
        packages.append({
            'package': name, 'directory': directory, 'group': directory.split('/')[2],
            'profiles': dict(sorted(reached_profiles.items())), 'production': in_production,
            'production_runtime': runtime,
            'consumers': reached.get(directory, []),
            'test_material': material.get(directory, {'roots': [], 'files': 0, 'test_attributes': 0}),
            'gates': sorted(gates[name], key=lambda gate: (gate['kind'], gate.get('shard') or '', gate['detail'])),
            'classification': classification, 'risk': risk,
        })

    retired = []
    present_roots = {path.name for path in (ROOT / 'test/foundation').iterdir() if path.is_dir()}
    for kind in ('retired', 'superseded'):
        for key in sorted(catalog[kind]):
            leaf = key.rsplit('/', 1)[1]
            stem = re.sub(r'-\d+(?:\.\d+)*(?:[-+].*)?$', '', leaf)
            assert stem not in present_roots or stem in names.values(), f'retired {key} keeps a test root'
            retired.append({'record': key, 'kind': kind})
    return {'schema': 1, 'generator': 'tools/rust-sources/foundation_coverage.py --write',
            'packages': packages, 'retired': retired}


def markdown(data):
    counts = defaultdict(int)
    risks = defaultdict(int)
    for package in data['packages']:
        counts[package['classification']] += 1
        if package['risk']:
            risks[package['risk']] += 1
    lines = [
        '# 基础组件测试覆盖映射', '',
        '由 `python3 tools/rust-sources/foundation_coverage.py --write` 生成，`--check` 核对；不要手工编辑。'
        '设计与分类规则见[测试覆盖闭环](foundation-test-coverage.md)。', '',
        '| 类别 | 数量 |', '| --- | ---: |',
        f'| 已执行 | {counts["executed"]} |',
        f'| 需要但未执行（高／中／低风险 {risks["high"]}／{risks["medium"]}／{risks["low"]}） | {counts["needed-not-executed"]} |',
        f'| 仅参考 | {counts["reference-only"]} |',
        f'| 活动合计 | {len(data["packages"])} |',
        f'| 已退役／被取代记录（无残留测试根） | {sum(r["kind"] == "retired" for r in data["retired"])}／'
        f'{sum(r["kind"] == "superseded" for r in data["retired"])} |', '',
        '执行种类：U 保留上游测试，P 项目新作测试或夹具，F 由已执行套件展开的伴随宏；I 仅编译或链接，不计为执行。'
        '消费者列出经 normal／build 边到达该包的一方入口及其 profile；特性取该消费者单独解析的结果。'
        '测试属性数为静态计数，不代表执行。', '',
    ]
    titles = {'needed-not-executed': '需要但未执行', 'executed': '已执行', 'reference-only': '仅参考'}
    for classification in ('needed-not-executed', 'executed', 'reference-only'):
        rows = [package for package in data['packages'] if package['classification'] == classification]
        order = {'high': 0, 'medium': 1, 'low': 2, None: 3}
        rows.sort(key=lambda package: (order[package['risk']], package['package']))
        lines += [f'## {titles[classification]}', '',
                  '| 包 | 风险 | profile（目标／角色／特性） | 一方消费者 | 测试材料 | 门禁 |',
                  '| --- | --- | --- | --- | --- | --- |']
        for package in rows:
            profile_text = '<br>'.join(
                f'{profile}：{value["target"].split("-")[0]}／{"、".join(value["roles"])}／'
                f'{", ".join(value["features"]) or "无"}'
                for profile, value in package['profiles'].items())
            consumer_text = '、'.join(sorted({name for row in package['consumers'] for name in row['consumers']})) or '—'
            material = package['test_material']
            material_text = f'{material["files"]} 文件／{material["test_attributes"]} 属性' if material['files'] else '无'
            gate_text = '<br>'.join(
                f'{gate["kind"]} {"、".join(Path(script).name for script in gate["scripts"]) or "—"}'
                for gate in package['gates']) or '—'
            risk = {'high': '高', 'medium': '中', 'low': '低', None: '—'}[package['risk']]
            lines.append(f'| `{package["package"]}` | {risk} | {profile_text} | {consumer_text} | '
                         f'{material_text} | {gate_text} |')
        lines.append('')
    return '\n'.join(lines)


def json_text(data):
    """One compact line per package or record so regenerated diffs stay per package."""
    def compact(value):
        return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(',', ':'))
    rows = ',\n'.join(compact(package) for package in data['packages'])
    retired = ',\n'.join(compact(record) for record in data['retired'])
    return (f'{{"generator":{compact(data["generator"])},"schema":{data["schema"]},\n'
            f'"packages":[\n{rows}\n],\n"retired":[\n{retired}\n]}}\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--write', action='store_true')
    mode.add_argument('--check', action='store_true')
    arguments = parser.parse_args()
    data = generate()
    outputs = {JSON_OUTPUT: json_text(data), MARKDOWN_OUTPUT: markdown(data)}
    if arguments.write:
        for path, text in outputs.items():
            path.write_text(text)
        print('Wrote ' + ', '.join(path.relative_to(ROOT).as_posix() for path in outputs))
        return
    for path, text in outputs.items():
        assert path.is_file() and path.read_text() == text, \
            f'{path.relative_to(ROOT)} differs from a fresh generation; run --write in the final integration'
    print(f'Foundation coverage map matches: {len(data["packages"])} active packages.')


if __name__ == '__main__':
    main()
