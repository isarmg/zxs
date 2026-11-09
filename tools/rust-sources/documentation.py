#!/usr/bin/env python3
"""Generate current dependency documentation without rewriting upstream evidence."""
import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
CATALOG = 'compliance/rust-sources.json'
ORIGINS = 'compliance/provenance/origins.json'
SUPPLEMENT = 'compliance/dependency-documents.json'
OUTPUT = 'docs/dependencies'
ORIGINALS = OUTPUT + '/originals'
GROUPS = {
    'code-generation': ('代码生成', 'Rust 语法、Token、通用宏与模板解析'),
    'collections': ('集合与内存结构', '集合、迭代器、智能指针、借用与零拷贝结构'),
    'concurrency': ('并发与同步', '执行器、任务调度、线程同步、通道与 Future'),
    'core-types': ('基础值类型', '数值、日期时间、标识符、标志位与基础值类型'),
    'cryptography': ('密码学', '哈希、认证、安全熵与密码学基础'),
    'data-storage': ('数据与存储', '序列化、编码、压缩、Schema 与 SQLite'),
    'diagnostics': ('诊断与追踪', '错误、日志、调试信息与追踪'),
    'networking': ('网络与协议', 'HTTP、路由、中间件、URI、套接字与 TLS'),
    'platform': ('系统与平台', '系统调用、动态库、CPU、文件系统与系统信息'),
    'text-unicode': ('文本与 Unicode', '文本检索、正则、Unicode 与区域数据'),
    'tooling': ('构建与命令行', 'C 绑定、原生构建、命令行、Shell 分词与终端样式'),
    'web-runtime': ('浏览器与 WASM', 'DOM、响应式界面、视图与 WASM 绑定'),
}
INTEGRATED = {
    'src/interface': ('docs/components/interface.md',),
    'src/naming': ('docs/components/naming.md',),
    'src/macros': ('docs/components/interface.md', 'docs/architecture/view-prop-parser.md'),
    'tools/web-bindings': ('docs/tools/web-bindings.md',),
}
# Only real, maintained guides belong here. Ordinary same-name component or
# architecture guides are discovered below; history and receipt prose are not.
GUIDES = {
    'blake2': ('docs/components/cryptographic-core.md',),
    'command-line': ('docs/architecture/command-line-reference/runtime/README.md',
                     'docs/architecture/command-line-reference/builder/README.md',
                     'docs/architecture/command-line-reference/lexer/README.md'),
    'command-derive': ('docs/architecture/command-line-reference/derive/README.md',),
    'uuid': ('docs/architecture/uuid-format-validation.md',),
    'error-definition': ('docs/architecture/error-definition-naming.md',),
    'error-definition-macros': ('docs/architecture/error-definition-naming.md',),
    'regex': ('docs/architecture/pattern-matching.md',),
    'unicode-normalizer': ('docs/architecture/unicode-data-families.md',),
    'unicode-properties': ('docs/architecture/unicode-data-families.md',),
    'serialization-derive': ('docs/components/data-serialization.md',),
    'reactive-state': ('docs/architecture/reactive-browser-runtime.md',),
    'view-renderer': ('docs/architecture/reactive-browser-runtime.md',),
    'dom-helpers': ('docs/components/interface.md',),
    'wasm-binding-macros': ('docs/tools/web-bindings.md',),
}
# Some upstream documents are large families. Prefer their useful entry points
# over dozens of example fragments or upstream development/history documents.
PREFERRED = {
    'axum': ('src/docs/extract.md', 'src/docs/response.md',
             'src/docs/middleware.md', 'src/docs/error_handling.md'),
    'clap': ('examples/tutorial_builder/01_quick.md', 'examples/tutorial_derive/01_quick.md'),
    'fancy-regex': ('docs/main.md', 'docs/syntax.md', 'docs/features.md', 'PERFORMANCE.md'),
    'sysinfo': ('md_doc/minimum_cpu_update_interval.md', 'md_doc/pid.md', 'md_doc/serde.md'),
}
USEFUL_NAMES = {
    'faq', 'design', 'implementation', 'performance', 'unicode', 'spec',
    'platform', 'compare', 'quoting_warning',
}
PROCESS = re.compile(r'(?:changelog|changes|news|release|contribut|conduct|security|'
                     r'policy|policies|maintenance|development|agents|issue_template|'
                     r'pull_request|migration|upgrade|reactor-refactor|crates-io|adding_new_platforms)', re.I)
# Identity-specific Rustdoc entries for sources without an independent guide.
# These paths follow the current implementation, not a retired crate facade.
INLINE = {
    'parking_lot_core': 'src/parking/mod.rs',
    'lock_api': 'src/api/mod.rs',
    'sqlx-core': 'src/query_engine/mod.rs',
    'sqlx-sqlite': 'src/sqlite/mod.rs',
    'percent-encoding': 'src/percent/mod.rs',
    'form_urlencoded': 'src/form/mod.rs',
    'wasm-bindgen-macro-support': 'src/lib.rs',
    'time-core': 'src/calendar_core/mod.rs',
    'jsonschema-regex': 'src/pattern/mod.rs',
    'cexpr': 'src/cexpr/mod.rs',
    'utf8parse': 'src/utf8/mod.rs',
}


class DocumentationError(ValueError):
    """Documentation coverage cannot safely be inferred from these inputs."""


def normal_path(root, relative, *, directory=False):
    """Accept repository-local regular paths, never traversal or symlink aliases."""
    if not isinstance(relative, str) or not relative:
        raise DocumentationError(f'Invalid repository path: {relative!r}')
    pure = PurePosixPath(relative)
    if pure.is_absolute() or '..' in pure.parts or pure.as_posix() != relative or '\\' in relative:
        raise DocumentationError(f'Unsafe repository path: {relative!r}')
    path = root / relative
    for parent in (path, *path.parents):
        if parent == root:
            break
        if parent.is_symlink():
            raise DocumentationError(f'Symlink is not a documentation target: {relative}')
    if not (path.is_dir() if directory else path.is_file()):
        raise DocumentationError(f'Missing {"directory" if directory else "file"}: {relative}')
    return path


def link(root, label, relative, *, directory=False):
    normal_path(root, relative, directory=directory)
    # Every generated page is a direct child of docs/dependencies.
    return f'[{label}](../../{relative})'


def cell(value):
    return str(value).replace('|', '\\|').replace('\n', ' ')


def current_sources(catalog, origins):
    original = {}
    for row in origins['packages']:
        key = row['origin_directory']
        if key in original:
            raise DocumentationError(f'Duplicate original identity: {key}')
        original[key] = row
    seen = set()
    result = []
    for kind, rows in (('active', catalog['packages'].items()),
                       ('merged', ((row['directory'], row) for row in catalog.get('merges', [])))):
        for directory, row in rows:
            key = row['origin_directory']
            if directory != row['directory']:
                raise DocumentationError(f'Current owner key mismatch: {directory}')
            if key in seen:
                raise DocumentationError(f'Duplicate current identity: {key}')
            if key not in original:
                raise DocumentationError(f'Unknown current identity: {key}')
            origin = original[key]
            for field in ('name', 'version', 'archive_url', 'archive_sha256'):
                if not row['upstream'].get(field) or row['upstream'][field] != origin['upstream'].get(field):
                    raise DocumentationError(f'Upstream identity mismatch ({field}): {key}')
            seen.add(key)
            result.append({'kind': kind, 'row': row, 'origin': origin})
    return result


def supplements(root, receipt, sources):
    if set(receipt) != {'schema', 'documents'} or receipt.get('schema') != 1 or not isinstance(receipt.get('documents'), list):
        raise DocumentationError('Unsupported supplemental documentation receipt')
    current = {source['row']['origin_directory']: source['origin'] for source in sources}
    indexed, seen, copied = defaultdict(list), set(), set()
    materials = {source['row']['origin_directory']: source['row']['license_materials'] for source in sources}
    registered = {path for source in sources
                  for path in source['origin']['central_original_documentation']}
    for document in receipt['documents']:
        required = {'origin_directory', 'upstream_name', 'upstream_version', 'archive_url',
                    'archive_sha256', 'original_path', 'path', 'sha256', 'bytes'}
        if not required <= document.keys() or document.keys() - required - {'license_materials'}:
            raise DocumentationError('Unknown or missing supplemental documentation fields')
        if type(document['bytes']) is not int or document['bytes'] < 0 or not re.fullmatch(r'[0-9a-f]{64}', document['sha256']):
            raise DocumentationError('Invalid supplemental document digest or size')
        key = document['origin_directory']
        if key not in current:
            raise DocumentationError(f'Supplement is not a current identity: {key}')
        origin = current[key]
        for field, upstream_field in (('upstream_name', 'name'), ('upstream_version', 'version'),
                                      ('archive_url', 'archive_url'), ('archive_sha256', 'archive_sha256')):
            if document.get(field) != origin['upstream'][upstream_field]:
                raise DocumentationError(f'Supplement identity mismatch ({field}): {key}')
        original = document['original_path']
        pure = PurePosixPath(original)
        if pure.is_absolute() or '..' in pure.parts or pure.as_posix() != original or '\\' in original:
            raise DocumentationError(f'Unsafe original document path: {original}')
        expected = f"{ORIGINALS}/{document['upstream_name']}-{document['upstream_version']}/{original}"
        relative = document['path']
        if relative != expected and relative not in registered:
            raise DocumentationError(f'Supplement path does not match identity or a registered exact-byte original: {relative}')
        if relative == expected:
            copied.add(relative)
        identity = (key, original)
        if identity in seen:
            raise DocumentationError(f'Duplicate supplemental identity/path: {key}: {original}')
        path = normal_path(root, relative)
        data = path.read_bytes()
        if len(data) != document['bytes'] or hashlib.sha256(data).hexdigest() != document['sha256']:
            raise DocumentationError(f'Supplement original bytes changed: {relative}')
        for license_path in document.get('license_materials', []):
            if license_path not in materials[key]:
                raise DocumentationError(f'Supplement license does not belong to current identity: {license_path}')
            normal_path(root, license_path)
        seen.add(identity)
        indexed[key].append(document)
    original_root = root / ORIGINALS
    present = set()
    if original_root.exists():
        normal_path(root, ORIGINALS, directory=True)
        for path in original_root.rglob('*'):
            relative = path.relative_to(root).as_posix()
            if path.is_symlink():
                raise DocumentationError(f'Symlink in supplemental originals: {relative}')
            if path.is_dir():
                continue
            normal_path(root, relative)
            present.add(relative)
    if present != copied:
        raise DocumentationError('Supplemental originals inventory mismatch: ' + ', '.join(sorted(present ^ copied)))
    return indexed


def group_for(directory):
    parts = PurePosixPath(directory).parts
    if parts[:2] == ('src', 'foundation') and len(parts) == 4:
        if parts[2] not in GROUPS:
            raise DocumentationError(f'Unknown foundation group: {parts[2]}')
        return parts[2]
    if directory in INTEGRATED:
        return 'integrated'
    raise DocumentationError(f'Unknown current implementation owner: {directory}')


def api_entry(root, directory):
    path = normal_path(root, directory + '/Cargo.toml')
    manifest = tomllib.loads(path.read_text(encoding='utf-8'))
    library = manifest.get('lib', {})
    relative = directory + '/' + library.get('path', 'src/lib.rs')
    if library or (root / relative).is_file():
        normal_path(root, relative)
    else:
        binaries = manifest.get('bin', [])
        if len(binaries) != 1 or 'path' not in binaries[0]:
            raise DocumentationError(f'No unambiguous current API entry: {directory}')
        relative = directory + '/' + binaries[0]['path']
        normal_path(root, relative)
    if not relative.endswith('.rs'):
        raise DocumentationError(f'Current API entry is not Rust source: {relative}')
    return manifest['package']['name'], relative


def current_guides(root, directory):
    if directory in INTEGRATED:
        paths = INTEGRATED[directory]
    elif PurePosixPath(directory).name in GUIDES:
        paths = GUIDES[PurePosixPath(directory).name]
    else:
        paths = tuple(f'docs/{section}/{PurePosixPath(directory).name}.md'
                      for section in ('components', 'architecture')
                      if (root / f'docs/{section}/{PurePosixPath(directory).name}.md').is_file())
    return [link(root, '当前维护说明' if index == 0 else '相关说明', path)
            for index, path in enumerate(paths)]


def choose_documents(root, origin, extra):
    if 'central_original_documentation' not in origin:
        raise DocumentationError(f'Missing original documentation inventory: {origin["origin_directory"]}')
    entries = []
    documents = []
    # Historical local aliases are explicit; new origins use their authenticated
    # upstream name/version without requiring a redundant display field.
    identity = origin.get('identity', origin['upstream']['name'] + '-' + origin['upstream']['version'])
    prefix = 'docs/upstream/' + identity + '/'
    for path in origin['central_original_documentation']:
        if not path.startswith(prefix):
            raise DocumentationError(f'Original document identity/path mismatch: {path}')
        tail = path.removeprefix(prefix)
        data = normal_path(root, path).read_bytes()
        original = tail.replace('upstream-github/', '.github/', 1)
        candidates = (original, original.removesuffix('.txt'))
        hashes = origin['original_files_sha256']
        if not any(hashes.get(candidate) == hashlib.sha256(data).hexdigest() for candidate in candidates):
            raise DocumentationError(f'Original document bytes do not match source receipt: {path}')
        documents.append((tail, path))
    # Exact-release supplemental copies supersede import-time documents with
    # the same original path, while the immutable imported bytes stay checked.
    replacements = {document['original_path'] for document in extra}
    documents = [(tail, path) for tail, path in documents if tail not in replacements]
    documents.extend((document['original_path'], document['path']) for document in extra)
    for tail, path in documents:
        if PROCESS.search(tail):
            continue
        filename = PurePosixPath(tail).name
        stem = PurePosixPath(filename).stem.lower()
        preferred = PREFERRED.get(origin['upstream']['name'], ())
        if '/' not in tail and filename.lower().startswith('readme'):
            priority = (0, 0)
        elif tail in preferred:
            priority = (1, preferred.index(tail))
        elif stem in USEFUL_NAMES:
            priority = (2, tail.count('/'))
        else:
            continue
        entries.append((priority, tail, path))
    return [(tail, path) for _, tail, path in sorted(entries)[:5]]


def fallback_entry(root, source, api):
    name = source['origin']['upstream']['name']
    relative = source['row']['directory'] + '/' + INLINE[name] if name in INLINE else api
    path = normal_path(root, relative)
    text = path.read_text(encoding='utf-8')
    if not re.search(r'(?m)^\s*(?://[/!]|/\*[*!]|#\!?\[doc\b)', text):
        raise DocumentationError(f'No standalone guide and no current Rustdoc fallback: {relative}')
    return relative


def legal_reference(root, source):
    row, origin = source['row'], source['origin']
    materials = row['license_materials']
    for path in materials:
        normal_path(root, path)
    parents = sorted({str(PurePosixPath(path).parent) for path in materials})
    if parents:
        licenses = '、'.join(link(root, '许可与通知' if index == 0 else '其他许可', path, directory=True)
                             for index, path in enumerate(parents))
    elif origin.get('requires_manual_notice_review'):
        licenses = '未附独立许可文件，见通知中的待复核项'
    else:
        raise DocumentationError(f'No license materials or explicit review state: {origin["origin_directory"]}')
    metadata = origin['central_original_metadata']
    for path in metadata:
        normal_path(root, path)
    candidates = sorted(metadata, key=lambda path: (not path.endswith('Cargo.toml.orig.txt'), path))
    provenance = candidates[0] if candidates else ORIGINS
    return licenses + '；' + link(root, '来源', provenance)


def render(root, catalog, origins, receipt):
    """Return all and only nonempty current group pages, validating every source."""
    root = root.resolve()
    normal_path(root, OUTPUT + '/README.md')
    sources = current_sources(catalog, origins)
    extra = supplements(root, receipt, sources)
    grouped = defaultdict(lambda: defaultdict(list))
    for source in sources:
        directory = source['row']['directory']
        grouped[group_for(directory)][directory].append(source)
    pages = {}
    for group, owners in sorted(grouped.items()):
        title, scope = ('业务组件内整合的依赖', '命名、共享界面、视图宏与 Web 绑定工具') if group == 'integrated' else GROUPS[group]
        count = sum(len(rows) for rows in owners.values())
        lines = [f'# {title}文档', '', '[依赖文档入口](README.md)', '',
                 f'{scope}。本页包含 {len(owners)} 个当前实现归属、{count} 个上游来源身份。', '',
                 '先读当前维护说明和本地 API/Rustdoc，再按需查阅与来源身份对应、随源码保留的说明。'
                 '这些资料可能描述已移除的平台、特性、独立包名或开发流程，不代表 ZXS 当前支持范围；合并来源不再提供独立 crate。', '',
                 '由 `python3 tools/rust-sources/documentation.py refresh` 生成；`verify` 检查覆盖、原件和链接目标。'
                 '这里只选择使用入口，不列上游变更日志、贡献流程或发布记录。', '']
        for directory, rows in sorted(owners.items()):
            name, api = api_entry(root, directory)
            guides = current_guides(root, directory)
            lines += [f'## {name}', '',
                      ' · '.join([link(root, '当前源码 / API', api),
                                  link(root, 'Cargo 清单', directory + '/Cargo.toml'), *guides]), '',
                      '| 上游来源身份 | 使用文档 | 许可与来源 |', '| --- | --- | --- |']
            for source in sorted(rows, key=lambda value: (value['kind'] != 'active', value['origin']['upstream']['name'], value['origin']['upstream']['version'])):
                origin = source['origin']
                identity = origin['upstream']['name'] + '@' + origin['upstream']['version']
                documents = choose_documents(root, origin, extra.get(origin['origin_directory'], []))
                if documents:
                    usage = '；'.join(link(root, tail, path) for tail, path in documents)
                    if extra.get(origin['origin_directory']):
                        usage += '（精确发布包原件）'
                    elif origin['upstream']['name'] == 'wasm-bindgen-cli':
                        usage += '（导入时保留的本地说明，非上游发布包文档）'
                else:
                    if origin['upstream']['name'] == 'thiserror-impl':
                        usage = ('未附独立文档或 Rustdoc；' + link(root, '内部实现源码', api) +
                                 '，按配套 ' + link(root, 'thiserror 使用文档', 'docs/upstream/thiserror-2.0.21/README.md') + ' 阅读')
                    else:
                        fallback = fallback_entry(root, source, api)
                        usage = '未附独立使用文档；' + link(root, '当前源码内 Rustdoc', fallback)
                        if origin['upstream']['name'] == 'walrus-macro':
                            usage += '（含生成类型的注释）；配套 ' + link(root, 'walrus 使用文档', 'docs/upstream/walrus-0.27.2/README.md')
                if source['kind'] == 'merged':
                    identity += '（合并来源）'
                lines.append('| ' + ' | '.join(map(cell, (identity, usage, legal_reference(root, source)))) + ' |')
            lines.append('')
        pages[f'{OUTPUT}/{group}.md'] = '\n'.join(lines)
    return pages


def synchronize(root, pages, command):
    """Refresh never deletes a page silently; removed owners require review."""
    allowed = {f'{OUTPUT}/{group}.md' for group in (*GROUPS, 'integrated')}
    if not pages.keys() <= allowed:
        raise DocumentationError('Refusing to write outside explicit generated dependency pages')
    normal_path(root, 'docs', directory=True)
    output = root / OUTPUT
    if output.exists() or output.is_symlink():
        normal_path(root, OUTPUT, directory=True)
    existing = {path.relative_to(root).as_posix() for path in output.glob('*.md') if path.name != 'README.md'}
    unexpected = existing - pages.keys()
    if unexpected:
        raise DocumentationError('Obsolete/unrecognized dependency pages; review and remove explicitly: ' + ', '.join(sorted(unexpected)))
    if command == 'refresh':
        output.mkdir(exist_ok=True)
        normal_path(root, OUTPUT, directory=True)
        for relative, content in pages.items():
            path = root / relative
            if path.is_symlink():
                raise DocumentationError(f'Refusing to overwrite symlink: {relative}')
            path.write_text(content, encoding='utf-8', newline='\n')
    else:
        for relative, content in pages.items():
            path = normal_path(root, relative)
            if path.read_text(encoding='utf-8') != content:
                raise DocumentationError(f'Dependency documentation drift: {relative}; run documentation.py refresh and review')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('refresh', 'verify'))
    args = parser.parse_args()
    try:
        def load(path):
            return json.loads(normal_path(ROOT, path).read_text(encoding='utf-8'))
        catalog = load(CATALOG)
        pages = render(ROOT, catalog, load(ORIGINS), load(SUPPLEMENT))
        synchronize(ROOT, pages, args.command)
    except (DocumentationError, KeyError, TypeError, OSError, ValueError) as error:
        print(f'Dependency documentation failed: {error}', file=sys.stderr)
        return 1
    print(f'Dependency documentation {args.command}: {len(pages)} pages; '
          f'{len(catalog["packages"])} active + {len(catalog.get("merges", []))} merged identities.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
