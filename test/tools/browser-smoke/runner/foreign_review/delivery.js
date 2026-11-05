// Test-only observation and delivery faults. Requests and successful response
// bytes always come from the actual fetch implementation; never fabricate them.
const originalFetch = window.fetch;
if (window.__zxsForeignReview) throw new Error('foreign review probe already installed');
const operations = new Map([
  ['POST /api/admin/storage/foreign/preview', 'source_preview'],
  ['POST /api/admin/storage/foreign/plans/preview', 'review_preview'],
  ['POST /api/admin/storage/foreign/plans/save', 'save'],
  ['GET /api/admin/storage/foreign/plans/status', 'status'],
  ['POST /api/admin/storage/foreign/plans/revalidate', 'revalidate'],
]);
const pending = new Map();
const probe = {
  records: [], holdNext: null, loseNextSave: false,
  release(id) {
    const held = pending.get(id);
    if (!held) throw new Error('no held original response for this record');
    pending.delete(id);
    held.record.delivery = 'released';
    held.resolve(held.response);
  },
  pendingIds() { return [...pending.keys()]; },
  restore() {
    if (window.fetch !== wrappedFetch && window.fetch !== originalFetch) {
      throw new Error('another owner replaced the foreign review fetch probe');
    }
    window.fetch = originalFetch;
    for (const id of [...pending.keys()]) probe.release(id);
    probe.holdNext = null;
    probe.loseNextSave = false;
  },
};
async function wrappedFetch(...args) {
  // Clone the live Request before observing it, including an explicit init
  // override. Forward the untouched original arguments to the actual fetch.
  const request = args[0] instanceof Request
    ? new Request(args[0].clone(), args[1])
    : new Request(new URL(args[0], location.href), args[1]);
  const url = new URL(request.url);
  if (url.origin !== location.origin || !url.pathname.startsWith('/api/admin/storage/foreign/')) {
    return originalFetch.apply(this, args);
  }
  const kind = operations.get(`${request.method} ${url.pathname}`) || 'forbidden';
  const record = {
    id: probe.records.length, kind, method: request.method, path: url.pathname,
    query: url.search, body: null, status: null, response: null, delivery: 'pending',
  };
  // Reserve the observation ID before awaiting a body clone: concurrent
  // requests must never alias a held response.
  probe.records.push(record);
  const requestText = await request.text();
  if (requestText) {
    try { record.body = JSON.parse(requestText); }
    catch { record.requestParseFailed = true; }
  }
  let response;
  try { response = await originalFetch.apply(this, args); }
  catch (error) {
    record.delivery = 'transport_failed';
    throw error;
  }
  record.status = response.status;
  const responseText = await response.clone().text();
  try { record.response = JSON.parse(responseText); }
  catch { record.responseParseFailed = true; }
  if (kind === 'save' && probe.loseNextSave && response.ok) {
    probe.loseNextSave = false;
    record.delivery = 'lost_after_actual_success';
    throw new TypeError('Foreign review fixture deliberately lost an actual saved-record response');
  }
  if (kind === probe.holdNext && response.ok) {
    probe.holdNext = null;
    record.delivery = 'held';
    return new Promise(resolve => pending.set(record.id, {record, response, resolve}));
  }
  record.delivery = 'delivered';
  return response;
}
window.__zxsForeignReview = probe;
window.fetch = wrappedFetch;
return true;
