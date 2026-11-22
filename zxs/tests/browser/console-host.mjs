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
  let focused, assigned, changed;
  const requests = [];

  class Element {
    constructor(name, namespace = null) { this.name = name; this.namespace = namespace; }
    attrs = new Map();
    children = [];
    parent = null;
    literal = "";
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
    querySelector(selector) {
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
      assert.equal(options.method, "GET");
      return new Promise((resolve) => requests.push({ path, signal: options.signal, resolve }));
    },
  };

  function navigate(hash) { window.location.hash = hash; changed(); }
  const flush = () => new Promise((resolve) => setImmediate(resolve));
  function reply(at, value, options = {}) {
    const payload = typeof value === "string" ? value : JSON.stringify(value);
    requests[at].resolve(new Response(payload, {
      status: options.status ?? 200,
      headers: { "content-type": options.mime ?? "application/json", "content-length": String(options.length ?? Buffer.byteLength(payload)) },
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
  reply(1, { ancestors: [{ id: parent, name: "设计文件" }],
    entries: [{ id: file, name: "<模型&>.zxs", kind: "file" }], next: cursor });
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
  console.log("Real WASM: original folder navigation, typed responses, stale cancellation and same-origin transport passed.");
} finally {
  await rm(directory, { recursive: true, force: true });
}
