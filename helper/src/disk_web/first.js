"use strict";
const pair = document.querySelector("#pair-form"), scope = document.querySelector("#scoped");
const code = document.querySelector("#code"), plan = document.querySelector("#plan"), status = document.querySelector("#status");
const refresh = document.querySelector("#preview"), proceed = document.querySelector("#continue"), cancel = document.querySelector("#cancel");
const hex = /^[0-9a-f]{64}$/;
const repair = document.querySelector("main").dataset.purpose === "copy-repair";
const route = repair ? "/api/copy-repair" : "/api/install";
const validByteCount = b => /^[1-9][0-9]{0,18}$/.test(b) && (b.length < 19 || b <= "9223372036854775807");
let auth = null, csrf = null, ticket = null;
// Match the server's maximum in first.rs, measured conservatively from before
// the preview request. Linux checks the actual ticket and expiry on handoff.
const TICKET_MS = 90_000;
function lockControls() {
    refresh.disabled = proceed.disabled = cancel.disabled = true;
}
async function post(path, body, authenticated = true) {
    const headers = {};
    if (body !== null) headers["Content-Type"] = "application/octet-stream";
    if (authenticated) {
        headers.Authorization = "Bearer " + auth;
        headers["X-Helper-CSRF"] = csrf;
    }
    const r = await fetch(path, {method: "POST", headers, body, cache: "no-store",
        credentials: "omit", redirect: "error"});
    if (!r.ok) throw new Error();
    return r.text();
}
async function fetchPreview() {
    const fields = (await post(route + "/preview", null)).split("\n");
    if (fields.length !== 3 || !hex.test(fields[2])) throw new Error();
    if (repair) {
        if (!["ext", "exFAT", "FAT32"].includes(fields[0]) || !validByteCount(fields[1])) throw new Error();
        plan.textContent = "终端已选一份 " + fields[1] + " 字节的 " + fields[0] + " 输入镜像和一个新工作副本位置。这里只发起终端确认，尚未读取镜像，也未创建文件。自动修复可能丢弃工作副本中的受损内容。";
    } else {
        if (!["new", "resume"].includes(fields[0]) || !/^(?:[1-9]|[1-5][0-9]|6[0-2])$/.test(fields[1])) throw new Error();
        plan.textContent = (fields[0] === "new" ? "新建空池" : "只续同一原单") + "：终端已选 1 个 SYSTEM、1 个独立回收盘和 " +
            fields[1] + " 个数据根。终端将显示实际原名和身份，本次预览不核盘内内容。";
    }
    return fields[2];
}
async function preview() {
    if (auth === null || refresh.disabled) return;
    lockControls();
    ticket = null;
    plan.textContent = status.textContent = "";
    const before = performance.now() + TICKET_MS;
    try {
        const value = await fetchPreview();
        ticket = {value, before};
        proceed.disabled = false;
        status.textContent = "已显示本次范围。预览票过期后请重新预览。";
    } catch (_) {
        status.textContent = "未取得有效预览。可重新预览或回原终端取消；网页未发起操作。";
    } finally {
        refresh.disabled = cancel.disabled = false;
    }
}
refresh.addEventListener("click", preview);
pair.addEventListener("submit", async e => {
    e.preventDefault();
    const button = pair.querySelector("button");
    if (button.disabled || auth !== null || !hex.test(code.value)) return;
    button.disabled = true;
    try {
        const proof = code.value; code.value = "";
        const fields = (await post("/api/pair", proof, false)).split("\n");
        if (fields.length !== 2 || !fields.every(x => hex.test(x)) || fields[0] === fields[1]) throw new Error();
        [auth, csrf] = fields;
        pair.hidden = true; scope.hidden = false; proceed.disabled = true;
        await preview();
    } catch (_) {
        status.textContent = "配对未确认。请核原终端和配对码；网页未发起操作。";
    } finally {
        if (auth === null) button.disabled = false;
    }
});
proceed.addEventListener("click", async () => {
    if (ticket === null) return;
    const proposed = ticket; ticket = null;
    if (performance.now() >= proposed.before) {
        proceed.disabled = true;
        status.textContent = "预览票已过期。请重新预览；网页未发起操作。";
        return;
    }
    lockControls();
    try {
        await post(route + "/request", proposed.value);
        status.textContent = "请求已交给本机终端，网页将关闭。请回原终端核对原件并决定；现在还没有开始。";
    } catch (_) {
        status.textContent = "未取得网页回执。先看原终端；收到本机确认提示才可继续，不另开新单。";
    } finally {
        auth = csrf = ticket = null;
    }
});
cancel.addEventListener("click", async () => {
    if (auth === null || cancel.disabled) return;
    lockControls();
    ticket = null;
    try {
        await post("/api/close", null);
        status.textContent = "网页已取消。";
    } catch (_) {
        status.textContent = "未取得取消回执。可回原终端直接取消。";
    } finally {
        auth = csrf = ticket = null;
    }
});
