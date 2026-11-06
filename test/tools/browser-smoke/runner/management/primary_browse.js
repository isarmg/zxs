// Transparent, bounded observation. Return the identical real Response; never
// manufacture success, rewrite bytes, suppress errors or invoke an API directly.
if (window.__zxsPrimaryBrowse) throw new Error('Primary browse observer already installed');
const original = window.fetch;
const state = window.__zxsPrimaryBrowse = {
    active: true, overflow: false, diskReads: [], entryReads: [], mutations: [],
    diskReady(name) {
        const row = this.diskReads.at(-1), disks = row?.body?.disks;
        return !this.overflow && row?.status === 200 && !row.parseError
            && Array.isArray(disks) && disks.length === 1
            && disks[0].slot_id === 'D01' && disks[0].name === name
            && disks[0].level === 1 && disks[0].online === true && disks[0].writable === true;
    },
    emptyRootReady() {
        const row = this.entryReads.at(-1), body = row?.body;
        return !this.overflow && row?.status === 200 && !row.parseError && row.rootRequest
            && body?.slot_id === 'D01' && body.path === '' && body.writable === true
            && !Object.hasOwn(body, 'backup') && !Object.hasOwn(body, 'preview')
            && Array.isArray(body.entries) && body.entries.length === 0
            && body.total_entries === '0' && body.directory_file_count === '0'
            && typeof body.snapshot === 'string' && body.snapshot.length > 0
            && body.previous_cursor === null && body.next_cursor === null;
    },
    restore() {
        this.active = false;
        window.fetch = original;
        delete window.__zxsPrimaryBrowse;
    }
};
const retain = (rows, record) => {
    if (!state.active) return;
    if (rows.length === 16) { state.overflow = true; return; }
    rows.push(record);
};
window.fetch = async function (...args) {
    const input = args[0];
    const url = new URL(typeof input === 'string' || input instanceof URL ? input : input.url, location.href);
    const method = (args[1]?.method || input.method || 'GET').toUpperCase();
    const own = url.origin === location.origin;
    if (own && url.pathname.startsWith('/api/') && !['GET', 'HEAD', 'OPTIONS'].includes(method)) {
        retain(state.mutations, {method, path: url.pathname});
    }
    const response = await Reflect.apply(original, this, args);
    if (!state.active || !own || method !== 'GET') return response;
    const rows = url.pathname === '/api/file-manager/disks' ? state.diskReads
        : url.pathname === '/api/file-manager/disks/D01/entries' ? state.entryReads : null;
    if (rows === null) return response;
    const query = url.searchParams;
    const rootRequest = query.getAll('path').length === 1 && query.get('path') === ''
        && query.getAll('query').length === 1 && query.get('query') === ''
        && !['cursor', 'directory_branch_id', 'expected_tree_id', 'selected_path', 'selected_entry_id']
            .some(key => query.has(key));
    const record = {status: response.status, path: url.pathname, rootRequest};
    try { record.body = await response.clone().json(); }
    catch (error) { record.parseError = String(error); }
    retain(rows, record);
    return response;
};
return true;
