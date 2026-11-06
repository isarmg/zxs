// Test-only observation of real UI reads. Navigation uses entries returned by
// the server, never an invented backup prefix or a synthetic successful page.
if (window.__zxsBackupBrowse) throw new Error('Backup browse probe already installed');
const original = window.fetch;
// Rust JSON Values can reorder object keys on the roundtrip through WebDriver.
// Compare the complete JSON structure, retaining array order and scalar types.
function same(a, b) {
    if (a === b) return true;
    if (a === null || b === null || typeof a !== 'object' || typeof b !== 'object') return false;
    const array = Array.isArray(a);
    if (array !== Array.isArray(b) || (array && a.length !== b.length)) return false;
    const keys = Object.keys(a);
    return keys.length === Object.keys(b).length
        && keys.every(key => Object.hasOwn(b, key) && same(a[key], b[key]));
}
const text = element => element?.textContent.trim();
const slots = ['P01', 'B01', 'C01'];
const modes = writable => writable ? ['Read/write directory', '读写目录'] : ['Read only', '只读'];
// This scenario follows a current namespace entry, not only equal content.
const identityFields = ['entry_id', 'file_id', 'repository_id', 'repository_path', 'version_id', 'node_id', 'size'];
const mutationLabels = [
    ['New folder', '新建文件夹'], ['Move', '移动'], ['Copy', '复制'],
    ['Rename', '重命名'], ['Delete', '删除'], ['Create folder branch', '创建目录分支'],
    ['Primary placement', '主盘规则'], ['Backup settings', '备份设置']
];
const detailMutations = [
    'Rename', '重命名', 'Delete', '删除', 'Edit file note', '编辑文件备注',
    'Branch from current content', '从当前内容创建分支', 'Merge branches', '合并分支',
    'Edit tags', '编辑标签', 'Edit note', '编辑备注',
    'Rename file branch', '重命名文件分支'
];
function diskRows() {
    return Array.from(document.querySelectorAll('.zxs-disk-table tbody tr'), row => {
        const button = row.querySelector('.zxs-disk-entry');
        return {slot: button?.dataset.slot, name: text(button), disabled: button?.disabled,
            level: Number(row.dataset.diskLevel), online: row.dataset.online,
            mode: text(row.cells[2])};
    });
}
function directoryView() {
    const table = document.querySelector('.zxs-file-table:not(.zxs-disk-table)');
    const section = document.querySelector('.zxs-file-manager');
    const controls = Array.from(document.querySelectorAll('.zxs-file-toolbar button'));
    return {ready: !!table && section?.getAttribute('aria-busy') === 'false',
        breadcrumbs: Array.from(document.querySelectorAll('.zxs-breadcrumb button'), text),
        rows: Array.from(table?.querySelectorAll('tbody tr') || [], row => ({
            name: text(row.querySelector('.zxs-file-name')), type: text(row.cells[1]),
            disabled: row.querySelector('.zxs-file-name')?.disabled,
            checked: row.querySelector('.zxs-entry-checkbox')?.checked,
            checkboxDisabled: row.querySelector('.zxs-entry-checkbox')?.disabled
        })),
        readonlyNotice: ['This disk directory is read only.', '此硬盘目录当前只读。']
            .some(label => Array.from(section?.querySelectorAll('p') || []).some(p => text(p) === label)),
        readonlyControls: mutationLabels.every(labels => {
            const matching = controls.filter(button => labels.includes(text(button)));
            return matching.length === 1 && matching[0].disabled;
        }) && document.querySelector('[data-zxs-set-directory-default]')?.disabled === true
            && table?.querySelector('thead input[type=checkbox]')?.disabled === true};
}
function validPath(path) {
    return typeof path === 'string' && (path === '' || path.split('/').every(part =>
        part.length > 0 && !['.', '..'].includes(part) && !/[\\\x00-\x1f]/.test(part)));
}
const p = window.__zxsBackupBrowse = {
    active: true, requests: [], blocked: [], pages: [], details: [], returns: [],
    initial: null, base: null, primaryChecks: 0, overflow: false, capturedBytes: 0,
    start() {
        const rows = diskRows();
        if (rows.length !== 3 || slots.some((slot, index) => {
            const matches = rows.filter(row => row.slot === slot);
            return matches.length !== 1 || !matches[0].name || matches[0].disabled !== false
                || matches[0].level !== index + 1 || matches[0].online !== 'true'
                || !modes(index === 0).includes(matches[0].mode);
        })) throw new Error('backup navigation needs the actual three-level fixture disk rows');
        p.initial = rows;
        return true;
    },
    mark() { p.check(); return p.requests.length; },
    check() {
        if (p.overflow || p.blocked.length) throw new Error('backup navigation exceeded its observation budget or tried a forbidden request');
        const failed = p.requests.find(row => row.error || (row.status !== null && row.status !== 200));
        if (failed) throw new Error(`backup navigation read failed: ${failed.kind} ${failed.slot || ''} HTTP ${failed.status}: ${failed.error || ''}`);
    },
    fresh(kind, slot, path, after) {
        p.check();
        const rows = p.requests.filter(row => row.sequence > after && row.kind === kind
            && row.slot === slot && row.path === path);
        if (rows.length > 1) throw new Error('backup navigation admitted duplicate reads for one click');
        return rows[0]?.complete ? rows[0] : null;
    },
    rootReady(after) {
        const read = p.fresh('disks', null, null, after);
        if (!read) return false;
        const disks = read.body?.disks, rows = diskRows();
        if (!Array.isArray(disks) || disks.length !== 3 || new Set(disks.map(disk => disk.slot_id)).size !== 3 || !p.initial)
            throw new Error('backup navigation did not receive the complete real disk list');
        return rows.length === disks.length && !document.querySelector('.zxs-breadcrumb')
            && document.querySelector('.zxs-file-manager')?.getAttribute('aria-busy') === 'false'
            && disks.every((disk, index) => {
                const before = p.initial.find(row => row.slot === disk.slot_id), row = rows[index];
                return before && row.slot === disk.slot_id && row.name === disk.name && row.name === before.name
                    && row.level === disk.level && row.level === before.level && disk.online === true
                    && row.disabled === false && row.online === 'true' && disk.writable === (disk.level === 1)
                    && modes(disk.writable).includes(row.mode);
            });
    },
    recordRoot(after) {
        if (!p.rootReady(after)) throw new Error('real All disks return is not rendered');
        const sequence = p.fresh('disks', null, null, after).sequence;
        const previous = p.returns.at(-1)?.sequence || 0;
        const pages = p.pages.filter(page => page.sequence > previous && page.sequence < sequence);
        const first = pages[0], last = pages.at(-1);
        if (!first || first.path !== '' || last.next.kind !== 'file'
            || pages.some((page, index) => page.slot !== first.slot || (index > 0
                && (pages[index - 1].next.kind !== 'directory' || pages[index - 1].next.path !== page.path))))
            throw new Error('All disks return lacks a continuous actual root-to-file UI navigation');
        if (first.slot !== 'P01' && !p.details.some(detail => detail.slot === first.slot
            && same(detail.file, last.next) && detail.historyAfter >= last.sequence && detail.historyAfter < sequence))
            throw new Error('backup return lacks its exact selected file detail');
        p.returns.push({sequence, slot: first.slot, pages: pages.map(page => page.sequence), rows: diskRows()});
        return true;
    },
    page(slot, path, after) {
        const read = p.fresh('entries', slot, path, after);
        if (!read) return null;
        const body = read.body, backup = slot !== 'P01';
        if (!read.fullQuery || !body || body.slot_id !== slot || body.path !== path || !validPath(path)
            || body.writable !== !backup || (backup ? body.backup !== true : Object.hasOwn(body, 'backup'))
            || Object.hasOwn(body, 'preview')
            || typeof body.snapshot !== 'string' || !body.snapshot || body.selected_entry !== null
            || body.previous_cursor !== null || body.next_cursor !== null
            || !Array.isArray(body.entries) || body.entries.length > 200
            || body.total_entries !== String(body.entries.length))
            throw new Error('backup navigation requires an exact complete real directory page');
        if (backup && (body.context !== null || body.directory_branches_supported !== false
            || body.default_branch_scope !== false || !Array.isArray(body.directory_branches)
            || body.directory_branches.length !== 0))
            throw new Error('backup directory exposed mutable repository context');
        const paths = new Set(), names = new Set();
        for (const entry of body.entries) {
            if (typeof entry.entry_id !== 'string' || !entry.entry_id)
                throw new Error('backup navigation entry identity is missing');
            if (backup && entry.kind === 'directory' && ['repository_id', 'repository_path', 'container', 'selected_branch_id']
                .some(key => Object.hasOwn(entry, key)))
                throw new Error('virtual backup directory exposed repository or file context');
            if (!entry.name || !validPath(entry.name) || entry.name.includes('/') || !validPath(entry.path)
                || entry.path !== (path ? `${path}/${entry.name}` : entry.name)
                || !['file', 'directory'].includes(entry.kind) || paths.has(entry.path) || names.has(entry.name)
                || (backup && (entry.backup !== true || entry.read_only !== true)))
                throw new Error('backup page contains a duplicate, foreign child or writable entry');
            paths.add(entry.path); names.add(entry.name);
        }
        return read;
    },
    pageReady(slot, path, after) {
        const read = p.page(slot, path, after);
        return !!read && p.matchesPage(read.body, directoryView());
    },
    matchesPage(body, view) {
        const disk = p.initial?.find(row => row.slot === body.slot_id);
        const crumbs = [disk?.name, ...(body.path ? body.path.split('/') : [])];
        return !!disk && view.ready && ['All disks', '所有硬盘'].includes(view.breadcrumbs[0])
            && same(view.breadcrumbs.slice(1), crumbs) && view.rows.length === body.entries.length
            && view.rows.every((row, index) => {
                const entry = body.entries[index];
                return row.name === entry.name && row.disabled === false && row.checked === false
                    && (entry.kind === 'directory' ? ['Folder', '文件夹'] : ['File', '文件']).includes(row.type)
                    && (body.backup !== true || row.checkboxDisabled === true);
            }) && (body.backup !== true || (view.readonlyNotice && view.readonlyControls));
    },
    step(slot, path, after) {
        const read = p.page(slot, path, after);
        if (!read || !p.matchesPage(read.body, directoryView())) throw new Error('directory response not rendered before navigation');
        const next = p.next(read.body);
        p.pages.push({sequence: read.sequence, slot, path, next, view: directoryView()});
        return next;
    },
    next(body) {
        if (body.path.split('/').at(-1) === 'browser-fixtures') {
            const [entry] = body.entries;
            if (body.entries.length !== 1 || entry.kind !== 'file' || entry.name !== 'browser-smoke.txt'
                || identityFields.some(key => typeof entry[key] !== 'string' || entry[key].length === 0))
                throw new Error('the seeded repository did not return its one identified real file');
            const backup = body.slot_id !== 'P01';
            if (entry.container !== true || entry.repository_path !== 'browser-smoke.txt'
                || entry.read_only !== backup || (backup ? entry.historical_only !== false : Object.hasOwn(entry, 'historical_only')))
                throw new Error('seeded entry is not the expected current file container');
            if (body.slot_id === 'P01') {
                if (p.base && !same(entry, p.base)) throw new Error('read-only navigation changed the primary file entry');
                if (!p.base) p.base = entry;
                p.primaryChecks++;
            } else if (!p.base || identityFields.some(key => entry[key] !== p.base[key])) {
                throw new Error('backup file differs from the actual primary identity or current version');
            }
            return entry;
        }
        const repository = body.entries.filter(entry => entry.kind === 'directory' && entry.name === 'browser-fixtures');
        if (repository.length === 1) return repository[0];
        // Only this declared single-chain fixture may walk a sole prefix.
        // This neither constructs nor asserts the product's desired prefix policy.
        if (body.entries.length === 1 && body.entries[0].kind === 'directory') return body.entries[0];
        throw new Error('real backup prefix is missing or ambiguous in this fixture');
    },
    detailReady(slot, file, after) {
        p.check();
        const history = p.requests.filter(row => row.sequence > after && row.kind === 'history'
            && row.file === file.file_id);
        const nodes = history.find(row => row.complete && row.body?.kind === 'nodes'
            && row.body.identity?.file_id === file.file_id
            && row.body.identity.context?.repository_id === file.repository_id
            && row.body.identity.context?.path === file.repository_path
            && row.body.nodes?.some(node => node.node_id === file.node_id && node.version_id === file.version_id));
        const section = document.querySelector('.zxs-entry-content');
        const controls = Array.from(section?.querySelectorAll('button') || []);
        const node = Array.from(section?.querySelectorAll('[data-node-id]') || [])
            .find(node => node.dataset.nodeId === file.node_id);
        const branches = history.find(row => row.complete && row.body?.kind === 'branches');
        return !!nodes && !!branches && history.every(row => row.complete
            && row.body?.identity?.file_id === file.file_id
            && row.body.identity.context?.repository_id === file.repository_id
            && row.body.identity.context?.path === file.repository_path
            && row.body.identity.history_root === nodes.body.identity.history_root)
            && typeof nodes.body.identity.history_root === 'string' && !!nodes.body.identity.history_root
            && text(section?.querySelector('.zxs-file-heading h2')) === file.name
            && text(section?.querySelector('.zxs-file-properties dd')) === `${slot}/${file.path}`
            && section?.querySelector('.zxs-entry-version-history')?.getAttribute('aria-busy') === 'false'
            && node?.dataset.bodyVersionId === file.version_id
            && section?.querySelector('[data-zxs-set-file-default]')?.disabled === true
            && !controls.some(button => !button.disabled && detailMutations.includes(button.getAttribute('aria-label') || text(button)));
    },
    recordDetail(slot, file, after) {
        if (!p.detailReady(slot, file, after)) throw new Error('backup file detail/history is not rendered read-only');
        if (p.details.some(detail => detail.slot === slot)) throw new Error('backup detail visited twice');
        p.details.push({slot, file, historyAfter: after});
        return true;
    },
    finish() {
        p.check();
        if (p.requests.some(row => !row.complete) || p.primaryChecks !== 2 || !same(p.details.map(detail => detail.slot), ['B01', 'C01'])
            || !same(p.returns.map(row => row.slot), ['P01', 'B01', 'C01', 'P01']) || !p.rootReady(p.returns.at(-1).sequence - 1))
            throw new Error('backup navigation did not visit both real levels and restore its baseline');
        return true;
    },
    snapshot() {
        return {scope: 'real fixture UI navigation and read-only metadata, not tier-local body reads, SMB or recursive writer proof',
            initial: p.initial, requests: p.requests, pages: p.pages, details: p.details,
            returns: p.returns, base: p.base, primaryChecks: p.primaryChecks,
            blocked: p.blocked, overflow: p.overflow, capturedBytes: p.capturedBytes};
    },
    restore() {
        p.active = false;
        if (window.fetch !== observed) throw new Error('backup browse observer was replaced before cleanup');
        window.fetch = original;
        delete window.__zxsBackupBrowse;
    }
};
async function observedBody(response, record) {
    const reader = response.clone().body.getReader();
    const decoder = new TextDecoder('utf-8', {fatal: true});
    let bytes = 0, raw = '';
    try {
        for (;;) {
            const {value, done} = await reader.read();
            if (done) break;
            bytes += value.byteLength;
            p.capturedBytes += value.byteLength;
            if (bytes > 1024 * 1024 || p.capturedBytes > 4 * 1024 * 1024) {
                p.overflow = true;
                throw new Error('backup navigation response recording budget exceeded');
            }
            raw += decoder.decode(value, {stream: true});
        }
        return raw + decoder.decode();
    } catch (error) {
        // A tee cancellation may wait for the original consumer. Never hold up
        // its real Response; keep any cleanup failure in the evidence instead.
        reader.cancel().catch(failure => { record.cancelError = failure.name; });
        throw error;
    } finally { reader.releaseLock(); }
}
async function observed(...args) {
    const input = args[0];
    const url = new URL(typeof input === 'string' || input instanceof URL ? input : input.url, location.href);
    const method = (args[1]?.method || input.method || 'GET').toUpperCase();
    const ownApi = url.origin === location.origin && url.pathname.startsWith('/api/');
    if (ownApi && (!['GET', 'HEAD'].includes(method) || /\/(content|download)(\/|$)/.test(url.pathname))) {
        p.blocked.push({method, path: url.pathname});
        throw new Error('backup browse guard blocked a mutation or content request before sending');
    }
    const entries = ownApi && /^\/api\/file-manager\/disks\/([^/]+)\/entries$/.exec(url.pathname);
    const history = ownApi && /^\/api\/file-manager\/files\/([^/]+)\/history(?:\/branches)?$/.exec(url.pathname);
    const kind = ownApi && url.pathname === '/api/file-manager/disks' ? 'disks'
        : entries ? 'entries' : history ? 'history' : null;
    let record = null;
    if (method === 'GET' && kind) {
        if (p.requests.length >= 64) { p.overflow = true; throw new Error('backup navigation observation request budget exceeded'); }
        const query = url.searchParams;
        record = {sequence: p.requests.length + 1, method, route: url.pathname, kind, slot: entries ? decodeURIComponent(entries[1]) : null,
            path: entries ? query.get('path') : null, file: history ? decodeURIComponent(history[1]) : null,
            fullQuery: !entries || (query.size === 3 && query.getAll('path').length === 1
                && query.get('query') === '' && query.get('limit') === '200'),
            status: null, complete: false};
        p.requests.push(record);
    }
    let response;
    try { response = await Reflect.apply(original, this, args); }
    catch (error) { if (record && p.active) record.error = 'original fetch rejected'; throw error; }
    if (record && p.active) {
        record.status = response.status;
        try {
            record.raw = await observedBody(response, record);
            record.body = JSON.parse(record.raw);
        } catch (error) { record.error = `response observation failed: ${error.name}`; }
        record.complete = true;
    }
    return response;
}
window.fetch = observed;
return true;
