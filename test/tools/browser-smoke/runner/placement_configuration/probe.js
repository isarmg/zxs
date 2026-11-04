const original = window.fetch;
if (window.__zxsPlacement) throw new Error('placement probe already installed');
const p = window.__zxsPlacement = {
  reads: [], writes: [], outcomes: [], holdRead: false, loseWrite: false, holdOutcome: false,
  pendingRead: null, pendingOutcome: null,
  restore() {
    window.fetch = original;
    p.pendingRead?.release();
    p.pendingOutcome?.release();
  }
};
window.fetch = async function (...args) {
  const request = args[0] instanceof Request ? args[0] : new Request(args[0], args[1]);
  const url = new URL(request.url, location.href);
  const method = request.method.toUpperCase();
  if (url.origin !== location.origin) return original.apply(this, args);
  const placement = /^\/api\/repositories\/[^/]+\/placement-configuration$/.test(url.pathname);
  const last = p.writes.at(-1);
  const outcome = method === 'GET' && last && url.pathname === '/api/operations/' + last.body.operation_id + '/outcome';
  if (!placement && !outcome) return original.apply(this, args);
  const record = {method, path: url.pathname, query: url.search, body: null, status: null, response: null};
  if (method === 'PUT') record.body = await request.clone().json();
  (outcome ? p.outcomes : method === 'GET' ? p.reads : p.writes).push(record);
  const response = await original.apply(this, args);
  record.status = response.status;
  record.response = await response.clone().json();
  if (placement && method === 'GET' && p.holdRead) {
    p.holdRead = false;
    return new Promise(resolve => {
      p.pendingRead = {release() {p.pendingRead = null; resolve(response);}};
    });
  }
  if (placement && method === 'PUT' && p.loseWrite && response.ok) {
    p.loseWrite = false;
    throw new TypeError('Placement browser probe: committed response was lost');
  }
  if (outcome && p.holdOutcome) {
    p.holdOutcome = false;
    return new Promise(resolve => {
      p.pendingOutcome = {release() {p.pendingOutcome = null; resolve(response);}};
    });
  }
  return response;
};
return true;
