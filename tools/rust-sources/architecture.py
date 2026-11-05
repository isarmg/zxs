#!/usr/bin/env python3
"""Generate the foundation consumer index from manifests and source receipts."""
import argparse
from collections import defaultdict
import json
from pathlib import Path
import sys
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent))
import dependency_edges

ROOT = Path(__file__).resolve().parents[2]
DOCUMENT = 'docs/architecture/foundation-consumers.md'


def cell(text):
    return str(text).replace('|', '\\|').replace('\n', ' ')


def manifests(root):
    paths = [root / 'Cargo.toml']
    for base in ('src', 'tools', 'rescue'):
        paths.extend((root / base).rglob('Cargo.toml'))
    return {path.relative_to(root).as_posix(): tomllib.loads(path.read_text(encoding='utf-8'))
            for path in sorted(set(paths))}


def consumer_edges(root, documents):
    """Keep dormant, target-specific and dev edges distinct from resolved use."""
    workspace = documents['Cargo.toml']['workspace']
    consumers = defaultdict(list)
    for relative, manifest in documents.items():
        if relative == 'Cargo.toml':
            continue  # workspace declarations are defaults, not consumers
        sections = [(None, manifest), *sorted(manifest.get('target', {}).items())]
        for target, section in sections:
            for kind in ('dependencies', 'build-dependencies', 'dev-dependencies'):
                for alias, declaration in sorted(section.get(kind, {}).items()):
                    dependency, base = dependency_edges.resolve(root, relative, manifest, workspace, alias, declaration)
                    destination = (base / dependency['path']).resolve().relative_to(root.resolve()).as_posix()
                    label = f"{Path(relative).parent.as_posix()} [{kind.removesuffix('-dependencies') if kind != 'dependencies' else 'normal'}"
                    if dependency.get('optional'):
                        label += ', optional'
                    if target:
                        label += ', ' + target
                    label += ']'
                    if dependency.get('package', alias) != alias:
                        label += ' alias=' + alias
                    consumers[destination].append(label)
    return {path: sorted(set(edges)) for path, edges in consumers.items()}


def document(root, catalog, policy):
    documents = manifests(root)
    consumers = consumer_edges(root, documents)
    lines = [
        '# Foundation 消费者与功能架构索引', '',
        '由 `python3 tools/rust-sources/architecture.py refresh` 生成；`verify` 拒绝过期内容。', '',
        '来源：当前全部 Cargo manifest、`workspaces.json` 和经 metadata 验证的 `compliance/rust-sources.json`。'
        '下列直接消费者包含 normal/build/dev、可选及目标条件声明；它们不等同于实际启用的边。'
        '功能列表来自整个工作区统一特性后的保守闭包，直接 dev 边虽不遍历，dev 激活的功能仍可能扩大闭包。'
        'runtime/compile 是维护角色，不证明最终链接或单消费者实际编译输入。', '',
        '```mermaid', 'flowchart TD',
        '  native[Linux amd64 产品] --> storage[auth / contract / platform / engine / server / client / rescue]',
        '  smb[Samba SDK 与协议] --> storage',
        '  browser[浏览器 WASM] --> web[web / interface]',
        '  web --> macros[macros 宿主过程宏]',
        '  storage --> foundation[foundation 定制基础实现]',
        '  web --> foundation', '  macros --> foundation',
        '  tools[Web 构建 / Unicode / 契约 / 浏览器校验工具] --> foundation',
        '  ring[ring 显式回归成员] --> foundation', '```', '',
        '物理目录按职责维护独立 crate 边界，当前名称与原始身份通过来源清单关联；跨网络、加密、Web 的共享库不按业务领域硬搬目录。'
        '项目需要的微型实现合并到真实消费者，来源身份继续登记。'
        '原生验收仅 `x86_64-unknown-linux-gnu`；浏览器为 `wasm32-unknown-unknown`。', '',
        '## 登记入口与保守功能闭包', '',
        '| 入口 | Cargo 根包目录 | 目标 | 来源数 / runtime / compile |', '| --- | --- | --- | --- |',
    ]
    for profile in policy['profiles']:
        for target in profile['targets']:
            resolution = next(r for r in catalog['resolutions'] if r['profile'] == profile['id'] and r['target'] == target)
            packages = resolution['packages']
            counts = [sum(role in value['roles'] for value in packages.values()) for role in ('runtime', 'compile')]
            lines.append('| ' + ' | '.join(map(cell, [profile['id'], ', '.join(profile.get('manifests', profile.get('packages', []))), target,
                                                       f"{len(packages)} / {counts[0]} / {counts[1]}"])) + ' |')
    lines += ['', '角色可重叠，计数不可相加。显式验证成员：' + ', '.join(policy.get('verification_members', [])) + '。', '',
              '## 每个保留来源的直接消费者、角色与功能', '',
              '| 当前来源目录 / 身份 | 全部声明的直接消费者 | 保守入口角色与统一 features |', '| --- | --- | --- |']
    sources = dict(catalog['packages'])
    for merge in catalog.get('merges', []):
        sources.setdefault(merge['directory'], merge)
    for path, package in sorted(sources.items()):
        usage = []
        for resolution in catalog['resolutions']:
            value = resolution['packages'].get(path)
            if value:
                usage.append(f"{resolution['profile']}@{resolution['target']}: {'+'.join(value['roles'])}; features={','.join(value['features']) or '(none)'}")
        lines.append('| ' + ' | '.join(map(cell, [f"{path} ({package['name']}@{package['version']})",
                                                       '; '.join(consumers.get(path, [])) or '工作区入口/显式验证成员', '; '.join(usage)])) + ' |')
    lines += ['', '## 合并来源身份', '', '| 上游身份 | 当前实现目录 |', '| --- | --- |']
    for merge in sorted(catalog.get('merges', []), key=lambda item: item['origin_directory']):
        lines.append('| ' + ' | '.join(map(cell, [f"{merge['upstream']['name']}@{merge['upstream']['version']}", merge['directory']])) + ' |')
    lines += ['', '## 锁文件与构建矩阵边界', '',
              '- 根 `Cargo.lock`：唯一活动产品/工具工作区，全部依赖为本地 path。',
              '- `.github/security-tools/cargo-audit-0.22.2.lock`：固定外部安全审计工具的安装闭包，受独立安装校验管理。',
              '- `compliance/provenance/packages/*/Cargo.lock.txt`：原始发布材料，只作为不可重写证据，不参与构建。',
              '- `.github/scripts/check-linux-*.py`：在临时独立 fixture 中重建保留后端组合及其临时锁，不能把它们当作新的产品工作区。'
              '矩阵继续拒绝继承 `CARGO_ENCODED_RUSTFLAGS` 和非空 `RUSTFLAGS`。',
              '- 三类受管锁文件各有职责；当前索引不修改安全工具锁或原始来源锁。', '',
              '未启用的声明仍要通过 `manage.py` 的休眠 feature/path 校验；裁剪一个身份必须先消除所有相关声明，显式退役并保留原始身份与法律材料。', '']
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('refresh', 'verify'))
    args = parser.parse_args()
    catalog = json.loads((ROOT / 'compliance/rust-sources.json').read_text(encoding='utf-8'))
    policy = json.loads((ROOT / 'tools/rust-sources/workspaces.json').read_text(encoding='utf-8'))
    expected = document(ROOT, catalog, policy)
    path = ROOT / DOCUMENT
    if args.command == 'refresh':
        path.write_text(expected, encoding='utf-8', newline='\n')
    elif not path.exists() or path.read_text(encoding='utf-8') != expected:
        print('Foundation consumer index drift; run architecture.py refresh and review.', file=sys.stderr)
        return 1
    print(f'Foundation consumer index {args.command}: {len(catalog["packages"])} active sources.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
