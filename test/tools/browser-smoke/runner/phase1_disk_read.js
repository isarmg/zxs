// Only delivery is interrupted. Both disk GETs reach the real server; no
// response data, access flags, timers or service limits are changed.
if (window.__zxsPhase1DiskRead) throw new Error('Disk-read probe already installed');
const original = window.fetch;
const held = [];
function spaceMatches(actual, value) {
    if (value === null) return ['Unavailable', '不可用'].includes(actual);
    const bytes = BigInt(value);
    if (bytes < 1024n) return actual === `${bytes} B`;
    // Match format::bytes: one u64 -> f64 conversion, exact power-of-two
    // divisions, then one decimal rounded to nearest with ties to even.
    // toFixed alone instead rounds exact half cases away from zero.
    const roundedBytes = Number(bytes), units = ['KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB'];
    let size = roundedBytes, divisor = 1n, index = 0;
    while (size >= 1024 && index < units.length) {
        size /= 1024;
        divisor *= 1024n;
        index++;
    }
    const scaled = BigInt(roundedBytes) * 10n, remainder = scaled % divisor;
    let tenths = scaled / divisor;
    if (remainder * 2n > divisor || (remainder * 2n === divisor && tenths % 2n !== 0n)) tenths++;
    return actual === `${tenths / 10n}.${tenths % 10n} ${units[index - 1]}`;
}
const p = window.__zxsPhase1DiskRead = {
    reads: [], statusHeld: false, holdStatus: true, closed: false,
    failure: 'Browser smoke: phase-one disk response delivery interrupted',
    rows() {
        return Array.from(document.querySelectorAll('.zxs-disk-table tbody tr'), row => {
            const button = row.querySelector('.zxs-disk-entry');
            return {slot: button.dataset.slot, name: button.textContent.trim(),
                disabled: button.disabled, online: row.dataset.online,
                level: Number(row.dataset.diskLevel), levelLabel: row.cells[1].textContent.trim(),
                mode: row.cells[2].textContent.trim(), observation: row.cells[3].textContent.trim(),
                space: row.cells[4].textContent.trim()};
        });
    },
    matches(rows, disks) {
        return rows.length === disks.length && rows.every((row, index) => {
            const disk = disks[index];
            const level = disk.level === 1 ? ['Level 1 · Primary', '第 1 级 · 主盘']
                : [`Level ${disk.level} · Backup`, `层级 ${disk.level} · 备份盘`];
            const mode = disk.writable ? ['Read/write directory', '读写目录'] : ['Read only', '只读'];
            const issue = disk.observation_failure?.stage;
            const observed = issue === 'seal'
                ? ['Unavailable (identity observation failed)', '不可访问（身份观测失败）']
                : issue === 'capacity'
                    ? (disk.online ? ['Online (capacity unavailable)', '在线（容量不可用）']
                        : ['Unavailable (capacity observation failed)', '不可访问（容量观测失败）'])
                    : (disk.online ? ['Online', '在线'] : ['Offline', '离线']);
            return row.slot === disk.slot_id && row.name === disk.name && row.disabled === !disk.online
                && row.online === String(disk.online) && row.level === disk.level
                && level.includes(row.levelLabel) && mode.includes(row.mode)
                && observed.includes(row.observation)
                && spaceMatches(row.space, disk.available_bytes);
        });
    },
    release(index, fail = false) {
        const record = p.reads[index];
        if (!record?.release) throw new Error('disk response is not held');
        record.fail = fail;
        record.release();
    },
    restore() {
        p.closed = true;
        p.holdStatus = false;
        window.fetch = original;
        for (const release of held) release();
        delete window.__zxsPhase1DiskRead;
    }
};
const pause = async record => {
    await new Promise(resolve => { record.release = resolve; held.push(resolve); });
};
window.fetch = async function(...args) {
    const input = args[0];
    const url = new URL(typeof input === 'string' ? input : input.url, location.href);
    const method = (args[1]?.method || input.method || 'GET').toUpperCase();
    const own = url.origin === location.origin && method === 'GET';
    const disk = own && url.pathname === '/api/file-manager/disks';
    const record = disk ? {status: null, body: null, delivered: false, fail: false} : null;
    if (record) p.reads.push(record);
    const response = await Reflect.apply(original, this, args);
    // One outstanding real health read excludes automatic recovery refreshes
    // from the explicit-button experiment. The original bytes are released.
    if (own && url.pathname === '/api/management/status' && p.holdStatus && !p.closed) {
        if (!response.ok) throw new Error('real management status failed during disk probe');
        p.statusHeld = true;
        await pause({});
    }
    if (!record) return response;
    record.status = response.status;
    record.body = await response.clone().json();
    if (!p.closed) await pause(record);
    record.delivered = true;
    if (record.fail && !p.closed) throw new TypeError(p.failure);
    return response;
};
return true;
