"use strict";
const pair = document.querySelector("#pair-form"), scope = document.querySelector("#scoped");
const code = document.querySelector("#code"), plan = document.querySelector("#plan"), status = document.querySelector("#status");
const proceed = document.querySelector("#continue"), cancel = document.querySelector("#cancel");
const hex = /^[0-9a-f]{64}$/;
let auth = null, csrf = null, ticket = null;
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
async function preview() {
    const fields = (await post("/api/install/preview", null)).split("\n");
    if (fields.length !== 3 || !["new", "resume"].includes(fields[0]) ||
        !/^(?:[1-9]|[1-5][0-9]|6[0-2])$/.test(fields[1]) || !hex.test(fields[2])) throw new Error();
    ticket = fields[2];
    plan.textContent = (fields[0] === "new" ? "新建空池" : "只续同一原单") + "：终端已选 1 个 SYSTEM、1 个独立回收盘和 " +
        fields[1] + " 个数据根。终端将显示实际原名和身份，本次预览不核盘内内容。";
    proceed.disabled = false;
}
pair.addEventListener("submit", async e => {
    e.preventDefault();
    if (auth !== null || !hex.test(code.value)) return;
    const button = pair.querySelector("button");
    button.disabled = true;
    try {
        const proof = code.value; code.value = "";
        const fields = (await post("/api/pair", proof, false)).split("\n");
        if (fields.length !== 2 || !fields.every(x => hex.test(x)) || fields[0] === fields[1]) throw new Error();
        [auth, csrf] = fields;
        pair.hidden = true; scope.hidden = false; proceed.disabled = true;
        await preview();
    } catch (_) {
        status.textContent = "未取得有效预览。请核原终端和配对码；网页未启动安装。";
    } finally {
        if (auth === null) button.disabled = false;
    }
});
proceed.addEventListener("click", async () => {
    if (ticket === null) return;
    const proposed = ticket; ticket = null; proceed.disabled = true; cancel.disabled = true;
    try {
        await post("/api/install/request", proposed);
        status.textContent = "请求已交给本机终端，网页将关闭。请回原终端核对原件并决定，不是安装完成。";
    } catch (_) {
        status.textContent = "未取得网页回执。先看原终端；收到本机确认提示才可继续，不另开新单。";
    } finally {
        auth = csrf = ticket = null;
    }
});
cancel.addEventListener("click", async () => {
    proceed.disabled = cancel.disabled = true;
    try {
        await post("/api/close", null);
        status.textContent = "网页已取消。";
    } catch (_) {
        status.textContent = "未取得取消回执。可回原终端直接取消。";
    } finally {
        auth = csrf = ticket = null;
    }
});
