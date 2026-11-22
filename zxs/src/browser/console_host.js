// First-party host for the Rust console. This module owns browser calls and
// ephemeral DOM handles; navigation and presentation stay in WebAssembly.
const SVG = "http://www.w3.org/2000/svg";
const MAX_BODY = 128 * 1024;
const SAME_ORIGIN = { mode: "same-origin", credentials: "same-origin", cache: "no-store", redirect: "error" };
const DIRECTORY = /^\/ui\/v1\/d\/[0-9a-f]{32}\/(?:-|[0-9a-f]{32})\/(?:-|[0-9a-f]{32})$/u;

async function directory(win, path, signal) {
  if (!DIRECTORY.test(path)) return [0, null];
  const response = await win.fetch(path, { ...SAME_ORIGIN, method: "GET", signal });
  if (response.status !== 200) return [response.status, null];
  const mime = response.headers.get("content-type")?.split(";", 1)[0];
  const length = response.headers.get("content-length");
  if (mime !== "application/json" || length === null || !/^[0-9]+$/u.test(length) ||
      Number(length) > MAX_BODY || response.body === null) return [0, null];

  const reader = response.body.getReader();
  const bytes = new Uint8Array(MAX_BODY);
  let used = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value.byteLength > MAX_BODY - used) {
        await reader.cancel();
        return [0, null];
      }
      bytes.set(value, used);
      used += value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  const value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, used)));
  return [200, value];
}

export default async function start() {
  const win = window;
  const doc = win.document;
  const held = new Map();
  const free = [];
  const decode = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  const encode = new TextEncoder();
  let next = 1;
  let wasm;
  let active;
  let page = null;

  // These slices exist only for the synchronous importing call. In particular,
  // fetch captures a string, not a view into growable WASM memory.
  const bytes = (ptr, length) => new Uint8Array(wasm.memory.buffer, ptr >>> 0, length >>> 0);
  const text = (ptr, length) => decode.decode(bytes(ptr, length));

  function put(value, ptr, capacity) {
    if (value === null) return -1;
    if (typeof value !== "string" || value.length > (capacity >>> 0)) return -2;
    const result = encode.encodeInto(value, bytes(ptr, capacity));
    return result.read === value.length ? result.written : -2;
  }

  function keep(node) {
    if (!(node instanceof win.Element)) return 0;
    const key = free.pop() ?? next++;
    if (key > 0xffffffff) throw new Error("console DOM handle limit");
    held.set(key, node);
    return key;
  }

  function element(key) {
    const node = held.get(key >>> 0);
    if (node === undefined) throw new Error("closed console DOM handle");
    return node;
  }

  function checked(action) {
    try { action(); return 1; }
    catch { return 0; }
  }

  function get(action) {
    try { return keep(action()); }
    catch { return 0; }
  }

  function error() {
    doc.getElementById("zxs-folder-load-error")?.removeAttribute("hidden");
  }

  function cancel() {
    active?.abort();
    active = undefined;
  }

  function request(ticket, path) {
    cancel();
    const controller = new AbortController();
    active = controller;
    void (async () => {
      let result;
      try { result = await directory(win, path, controller.signal); }
      catch { result = [0, null]; }
      if (active !== controller) return; // The Rust ticket/hash also have to match.
      active = undefined;
      page = result[1];
      try { wasm.zxs_directory_result(ticket, result[0]); }
      catch { error(); }
      finally { page = null; }
    })();
  }

  const group = (at) => at === 0 ? page?.ancestors : at === 1 ? page?.entries : undefined;
  const imports = {
    find: (ptr, len) => get(() => doc.getElementById(text(ptr, len))),
    query: (id, ptr, len) => get(() => element(id).querySelector(text(ptr, len))),
    first: (id) => get(() => element(id).firstElementChild),
    share_count: (id) => element(id).querySelectorAll("[data-zxs-share]").length,
    share_at: (id, at) => get(() => element(id).querySelectorAll("[data-zxs-share]").item(at >>> 0)),
    make: (ptr, len, svg) => get(() => svg ? doc.createElementNS(SVG, text(ptr, len)) : doc.createElement(text(ptr, len))),
    release: (id) => { const key = id >>> 0; if (held.delete(key)) free.push(key); },
    place: (parent, child, prepend) => checked(() => {
      const at = element(parent), node = element(child);
      if (prepend) at.insertBefore(node, at.firstChild);
      else at.appendChild(node);
    }),
    write_text: (id, ptr, len) => checked(() => { element(id).textContent = text(ptr, len); }),
    write_attr: (id, ptr, len, value, size, remove) => checked(() => {
      const at = element(id), name = text(ptr, len);
      if (remove) at.removeAttribute(name);
      else at.setAttribute(name, text(value, size));
    }),
    move_focus: (id) => checked(() => element(id).focus()),
    read_attr: (id, ptr, len, out, cap) => put(element(id).getAttribute(text(ptr, len)), out, cap),
    read_text: (id, out, cap) => put(element(id).textContent, out, cap),
    read_location: (out, cap) => {
      const hash = win.location.hash;
      // A valid route fits in 107 ASCII bytes. A huge hash is still an
      // invalid directory route or an unrelated fragment, never a prefix.
      const bounded = hash.length <= 128 ? hash : hash.startsWith("#zxs-d/") ? "#zxs-d" : "#other";
      return put(bounded, out, cap);
    },
    to_sign_in: () => checked(() => win.location.assign("/login")),
    begin_read: (ticket, ptr, len) => request(ticket, text(ptr, len)),
    cancel_read: cancel,
    row_count: (at) => { const rows = group(at); return Array.isArray(rows) ? rows.length : -2; },
    read_row: (at, index, which, out, cap) => {
      if (at === 2) return put(page?.next, out, cap);
      const row = group(at)?.[index >>> 0];
      const value = which === 0 ? row?.id : which === 1 ? row?.name : row?.kind;
      return typeof value === "string" ? put(value, out, cap) : -2;
    },
  };

  const response = await win.fetch(new URL("./zxs_browser_bg.wasm", import.meta.url), SAME_ORIGIN);
  if (response.status !== 200) throw new Error("could not load the ZXS console");
  const { instance } = await WebAssembly.instantiateStreaming(response, { zxs_console: imports });
  wasm = instance.exports;
  if (wasm.zxs_mount() !== 1) throw new Error("incomplete ZXS console");
  win.addEventListener("hashchange", () => {
    try { wasm.zxs_hash_changed(); }
    catch { error(); }
  });
}
