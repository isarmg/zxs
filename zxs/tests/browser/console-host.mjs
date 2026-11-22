// Synthetic browser APIs around the actual staged WASM. No server or network.
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { pathToFileURL } from "node:url";

const stage = process.argv[2];
assert.ok(stage, "pass the qualified browser stage directory");
const directory = await mkdtemp(join(tmpdir(), "zxs-console-test-"));
try {
  const host = join(directory, "console.mjs");
  await writeFile(host, await readFile(join(stage, "zxs_browser.js")));
  const bytes = await readFile(join(stage, "zxs_browser_bg.wasm"));
  const compiled = new WebAssembly.Module(bytes);
  assert.deepEqual([...new Set(WebAssembly.Module.imports(compiled).map((item) => item.module))], ["zxs_console"]);
  const { default: start } = await import(pathToFileURL(host));
  const share = "1".repeat(32), parent = "2".repeat(32), file = "3".repeat(32), cursor = "4".repeat(32);
  const panelIds = ["zxs-files", "zxs-shares", "zxs-versions", "zxs-credentials"];
  let focused, assigned, changed, submitted;
  const requests = [];
  const canonical = (x) => Buffer.from(x, "utf8").toString("hex");
  const position = "5".repeat(32), csrf = "06".repeat(32), scope = "07".repeat(32);
  const entries = [{ id: file, name: "<模型&>.zxs", kind: "file", position }];
  const current = { ancestors: [{ id: parent, name: "设计文件" }], entries, next: cursor, scope, writable: true };

  class Element {
    constructor(name, namespace = null) { this.name = name; this.namespace = namespace; }
    attrs = new Map();
    children = [];
    parent = null;
    literal = "";
    currentValue = null;
    get value() { return this.currentValue ?? this.getAttribute("value") ?? ""; }
    set value(v) { this.currentValue = v; }
    get firstChild() { return this.children[0] ?? null; }
    get firstElementChild() { return this.firstChild; }
    get textContent() { return this.literal + this.children.map((node) => node.textContent).join(""); }
    set textContent(value) {
      for (const child of this.children) child.parent = null;
      this.children = [];
      this.literal = value;
    }
    getAttribute(name) { return this.attrs.get(name) ?? null; }
    setAttribute(name, value) { this.attrs.set(name, value); }
    removeAttribute(name) { this.attrs.delete(name); }
    insertBefore(node, before) {
      if (node.parent !== null) node.parent.children.splice(node.parent.children.indexOf(node), 1);
      const at = before === null ? this.children.length : this.children.indexOf(before);
      assert.ok(at >= 0, "insertion requires an original child");
      this.children.splice(at, 0, node);
      node.parent = this;
    }
    appendChild(node) { this.insertBefore(node, null); }
    walk() { return this.children.flatMap((node) => [node, ...node.walk()]); }
    addEventListener(name, callback) {
      assert.equal(name, "submit");
      submitted = callback;
    }
    querySelector(selector) {
      if (selector === "input[name=label]") {
        return this.walk().find((el) => el.name === "input" && el.getAttribute("name") === "label") ?? null;
      }
      if (selector === "[role=status]") {
        return this.walk().find((el) => el.getAttribute("role") === "status") ?? null;
      }
      const href = /^a\[href="([^"]+)"\]$/u.exec(selector)?.[1];
      return this.walk().find((el) => el.name === "a" && el.getAttribute("href") === href) ?? null;
    }
    querySelectorAll(selector) {
      assert.equal(selector, "[data-zxs-share]");
      const values = this.walk().filter((el) => el.attrs.has("data-zxs-share"));
      return { length: values.length, item: (at) => values[at] ?? null };
    }
    focus() { focused = this; }
  }

  const nodes = new Map();
  function fixture(id, name = "div") {
    const el = new Element(name);
    nodes.set(id, el);
    return el;
  }
  const nav = fixture("zxs-console-nav"), content = fixture("zxs-folder-content");
  fixture("zxs-console-secondary");
  const error = fixture("zxs-folder-load-error");
  error.setAttribute("hidden", "");
  const paint = new Element("svg", "http://www.w3.org/2000/svg");
  fixture("zxs-file-corner-source").appendChild(paint);
  const roots = fixture("zxs-browser-roots");
  roots.setAttribute("data-zxs-next", cursor);
  roots.setAttribute("data-zxs-csrf", csrf);
  const root = new Element("span");
  root.textContent = "项目 <&>";
  root.setAttribute("data-zxs-share", share);
  roots.appendChild(root);
  const account = fixture("zxs-account-link", "a");
  account.setAttribute("href", "#zxs-credentials");
  nav.appendChild(account);
  for (const panel of panelIds) {
    fixture(panel).setAttribute("aria-labelledby", panel + "-title");
    fixture(panel + "-title", "h2");
    if (panel === "zxs-shares" || panel === "zxs-versions") {
      const link = new Element("a");
      link.setAttribute("href", "#" + panel);
      nav.appendChild(link);
    }
  }

  globalThis.window = {
    Element,
    document: {
      getElementById: (id) => nodes.get(id) ?? null,
      createElement: (name) => new Element(name),
      createElementNS: (ns, name) => new Element(name, ns),
    },
    location: { hash: "", assign: (url) => { assigned = url; } },
    addEventListener(name, callback) { assert.equal(name, "hashchange"); changed = callback; },
    fetch(path, options) {
      for (const [k, v] of Object.entries({ mode: "same-origin", credentials: "same-origin", cache: "no-store", redirect: "error" })) {
        assert.equal(options[k], v);
      }
      if (path instanceof URL) {
        assert.equal(path.protocol, "file:");
        assert.equal(basename(path.pathname), "zxs_browser_bg.wasm");
        return Promise.resolve(new Response(bytes, { headers: { "content-type": "application/wasm" } }));
      }
      assert.ok(options.method === "GET" || options.method === "POST");
      let form = null;
      if (options.method === "POST") {
        assert.equal(options.headers["content-type"], "application/x-www-form-urlencoded");
        assert.ok(path === "/ui/v1/df" || path === "/ui/v1/labels");
        form = Object.fromEntries(new URLSearchParams(options.body));
        assert.equal(form.csrf, csrf);
      }
      return new Promise((resolve) => requests.push({ path, method: options.method, form, signal: options.signal, resolve }));
    },
  };

  function navigate(hash) { window.location.hash = hash; changed(); }
  const flush = () => new Promise((resolve) => setImmediate(resolve));
  function reply(at, value, options = {}) {
    const payload = typeof value === "string" ? value : JSON.stringify(value);
    const content = options.status === 204 ? null : payload;
    requests[at].resolve(new Response(content, {
      status: options.status ?? 200,
      headers: { "content-type": options.mime ?? "application/json", "content-length": String(options.length ?? Buffer.byteLength(content ?? "")) },
    }));
  }

  await start();
  const links = () => content.walk().filter((el) => el.name === "a").map((el) => el.getAttribute("href"));
  assert.equal(nav.firstChild.getAttribute("href"), "#zxs-files");
  assert.ok(links().includes(`#zxs-d/${share}/-/-`) && links().includes(`/?after=${cursor}#zxs-files`));
  assert.ok(content.textContent.includes("项目 <&>") && content.walk().includes(paint));
  assert.ok(content.walk().some((el) => el.name === "svg" && el.namespace === paint.namespace));
  assert.equal(error.getAttribute("hidden"), "");

  const base = `#zxs-d/${share}/${parent}/-`;
  navigate(`#zxs-d/${share}/-/-`);
  navigate(base);
  assert.equal(requests[0].signal.aborted, true);
  assert.deepEqual(requests.map((r) => r.path), [`/ui/v1/d/${share}/-/-`, `/ui/v1/d/${share}/${parent}/-`]);
  assert.equal(content.getAttribute("aria-busy"), "true");
  assert.equal(focused, nodes.get("zxs-files-title"));
  // A late success after an ignored abort must never overwrite the selected folder.
  reply(0, { ancestors: [], entries: [{ id: file, name: "旧文件", kind: "file" }], next: null });
  await flush();
  assert.ok(!content.textContent.includes("旧文件"));
  assert.equal(content.getAttribute("aria-busy"), "true");

  // The skip link doesn't invalidate an already selected directory.
  navigate("#zxs-main-content");
  reply(1, current);
  await flush();
  assert.equal(content.getAttribute("aria-busy"), null);
  assert.ok(content.textContent.includes("设计文件") && content.textContent.includes("<模型&>.zxs"));
  assert.ok(links().includes(`/versions/history?file=${file}`) && links().includes(`/versions?file=${file}`));
  assert.ok(links().includes(`#zxs-d/${share}/${parent}/${cursor}`));
  assert.ok(content.walk().includes(paint));

  navigate("#zxs-credentials");
  assert.equal(account.getAttribute("aria-current"), "page");
  assert.equal(nodes.get("zxs-shares").getAttribute("hidden"), "");
  navigate(`#zxs-d/${share}/%2e/-`);
  assert.equal(requests.length, 2);
  assert.ok(content.textContent.includes("目录链接无效"));

  navigate(base);
  reply(2, { ancestors: [], entries: [], next: null }); // the original parent is required
  await flush();
  assert.ok(content.textContent.includes("读取失败"));
  navigate(base);
  reply(3, "{}".padEnd(128 * 1024 + 1), { length: 2 }); // header alone isn't a body bound
  await flush();
  assert.ok(content.textContent.includes("读取失败"));
  navigate(base);
  reply(4, {}, { status: 403 });
  await flush();
  assert.ok(content.textContent.includes("无法打开此目录"));
  navigate(base);
  reply(5, {}, { status: 401 });
  await flush();
  assert.equal(assigned, "/login");

  function submit(action, input = undefined) {
    const form = content.walk().find((el) => el.getAttribute("data-zxs-action") === action && el.name === "form");
    assert.ok(form, `rendered ${action} form`);
    if (input !== undefined) form.querySelector("input[name=label]").value = input;
    let prevented = false;
    submitted({ target: form, preventDefault() { prevented = true; } });
    assert.equal(prevented, true);
  }
  navigate(base);
  reply(6, current);
  await flush();
  submit("filter", "中".repeat(100)); // 100 codepoints, 300 UTF-8 bytes; no truncation.
  assert.ok(content.textContent.includes("255"));
  assert.equal(requests.length, 7);
  const exact = "图纸 <&>";
  const selected = canonical(exact), filtered = `${base}/${selected}`;
  submit("filter", exact);
  assert.equal(window.location.hash, filtered);
  changed();
  assert.equal(requests[7].path, "/ui/v1/df");
  assert.deepEqual(requests[7].form, { csrf, share, parent, after: "-", label: selected });
  const child = "8".repeat(32);
  reply(7, { ...current, entries: [{ id: child, name: "子目录", kind: "directory" }, ...entries] });
  await flush();
  assert.ok(links().includes(base) && links().includes(`#zxs-d/${share}/${child}/-/${selected}`));
  assert.ok(links().includes(`#zxs-d/${share}/${parent}/${cursor}/${selected}`));

  submit("open");
  assert.deepEqual(requests[8].form, { csrf, share, parent, file, position, scope, action: "list", label: "-" });
  const labels = Array.from({ length: 31 }, (_, i) => canonical(`caption ${String(i).padStart(2,"0")}`));
  labels.push(selected);
  reply(8, { labels, next: selected });
  await flush();
  assert.ok(content.walk().some((el) => el.name === "bdi" && el.textContent === exact));
  submit("next");
  assert.equal(requests[9].form.label, selected);
  reply(9, { labels: [], next: null });
  await flush();
  assert.ok(content.textContent.includes("此页没有更多标签"));
  submit("add", exact);
  assert.equal(requests[10].form.action, "add");
  assert.equal(requests[10].form.label, selected);
  assert.equal(requests[10].signal, undefined); // Navigation cannot undo a sent edit.
  reply(10, null, { status: 204 });
  await flush();
  assert.equal(requests[11].form.action, "list"); // Only a read follows success.
  reply(11, { labels: [selected], next: null });
  await flush();
  submit("remove");
  assert.equal(requests[12].form.action, "remove");
  assert.equal(requests[12].form.label, selected);
  reply(12, null, { status: 204 });
  await flush();
  assert.equal(requests[13].path, "/ui/v1/df"); // Changed membership reopens folder on the first page.
  reply(13, { ...current, entries: [], next: null });
  await flush();
  assert.ok(content.textContent.includes("没有匹配标签") && !content.textContent.includes("模型"));

  navigate(base);
  reply(14, current);
  await flush();
  submit("open");
  reply(15, { labels: [], next: null });
  await flush();
  submit("add", "late");
  navigate("#zxs-credentials");
  reply(16, null, { status: 204 });
  await flush();
  assert.equal(requests.length, 17); // Stale result must not start a read or rewrite another view.
  assert.ok(!content.textContent.includes("正在保存标签"));
  navigate(base);
  reply(17, { ...current, writable: false });
  await flush();
  submit("open");
  reply(18, { labels: [selected], next: null });
  await flush();
  assert.ok(content.textContent.includes(exact));
  assert.ok(!content.walk().some((el) => ["add", "remove"].includes(el.getAttribute("data-zxs-action"))));
  console.log("Real WASM: folders, exact label filter/edit/paging, stale results, read-only controls and same-origin transport passed.");
} finally {
  await rm(directory, { recursive: true, force: true });
}
