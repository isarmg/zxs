"use strict";
const byId = id => document.getElementById(id);
let csrf = null;
let busy = false;
let stopping = false;
const maxSelectionBytes = 60 * 1024;
function error(message) { byId("error").textContent = message; byId("error").hidden = !message; }
function setBusy(value) { busy = value; byId("check").disabled = value || stopping || !csrf; byId("selection").disabled = value || stopping; byId("selection-file").disabled = value || stopping; }
function resetResults() { byId("results").hidden = true; byId("copies").replaceChildren(); byId("report").textContent = ""; error(""); }
async function status() {
  const response = await fetch("/api/status", {cache:"no-store", credentials:"omit"});
  if (!response.ok) throw new Error(`无法连接辅助工具（HTTP ${response.status}）。请使用主机的 IP 和实际端口直接打开，不使用 DNS 名称或代理。`);
  const state = await response.json(); csrf = state.csrf; stopping = state.stopping;
  byId("connection").textContent = stopping ? "辅助工具正在等待已开始的读取结束并退出。" : `${state.name} ${state.version} · 独立只读模式 · 无登录 · ${state.busy ? "已有磁盘读取任务" : "未启动磁盘任务"}`;
  setBusy(busy);
}
async function post(path, body) {
  const response = await fetch(path, {method:"POST", credentials:"omit", cache:"no-store", headers:{"Content-Type":"application/json","X-ZXS-Helper-CSRF":csrf}, body:JSON.stringify(body)});
  if (!response.ok) {
    let message = `辅助工具拒绝请求（HTTP ${response.status}）`;
    try { const value = await response.json(); if (value.error && typeof value.error.message === "string") message += `：${value.error.message}`; } catch { /* Retain the concrete HTTP failure when no JSON diagnostic was delivered. */ }
    throw new Error(message);
  }
  return response;
}
function save(blob, name) { const link = document.createElement("a"); const url = URL.createObjectURL(blob); link.href = url; link.download = name; document.body.append(link); link.click(); link.remove(); setTimeout(() => URL.revokeObjectURL(url), 1000); }
function appendCell(row, text) { const cell = document.createElement("td"); cell.textContent = text; row.append(cell); return cell; }
function render(report, selection) {
  const captured = JSON.parse(JSON.stringify(selection)); captured.known_checkpoint = report.checkpoint;
  const labels = {verified:"正文已核验", temporarily_unavailable:"正文暂不可用", corrupt:"正文损坏"};
  const summaries = {no_published_content:"完整读取了元数据，但本盘尚无已发布正文。", complete_success:"本次所选快照保留的正文均通过长度与摘要检查。", content_issues:"检查发现缺失或损坏正文；未修复。仍可单独导出本次可核验的版本。"};
  byId("summary").textContent = `${summaries[report.state]} 共 ${report.copies.length} 个版本。`;
  byId("report").textContent = JSON.stringify(report, null, 2);
  byId("copies").replaceChildren();
  for (const copy of report.copies) {
    const row = document.createElement("tr");
    appendCell(row, `${copy.history_id} / ${copy.file_id}`);
    const version = appendCell(row, copy.commit.slice(0, 12)); version.title = copy.commit;
    appendCell(row, String(copy.expected_bytes)); appendCell(row, labels[copy.content.status]);
    const actions = appendCell(row, "");
    if (copy.content.status === "verified") {
      const download = document.createElement("button"); download.type = "button"; download.textContent = "核验并下载";
      download.addEventListener("click", async () => {
        if (busy || stopping) return;
        error(""); setBusy(true); download.disabled = true;
        try {
          const response = await post("/api/authority/export", {selection:captured, commit:copy.commit, writers_stopped:byId("writers-stopped").checked});
          if (response.headers.get("x-zxs-commit") !== copy.commit || response.headers.get("x-zxs-body-digest") !== copy.body) throw new Error("下载响应与所选版本不符；未保存文件。");
          const body = await response.blob();
          if (body.size !== copy.expected_bytes) throw new Error("下载字节数与已验证版本不符；未保存文件。");
          save(body, `zxs-${copy.commit}.bin`);
        } catch (failure) { error(failure.message); } finally { setBusy(false); download.disabled = false; }
      }); actions.append(download);
      const receipt = document.createElement("button"); receipt.type = "button"; receipt.className = "secondary"; receipt.textContent = "保存核验信息";
      receipt.addEventListener("click", () => save(new Blob([JSON.stringify({format:"zxs-helper-local-export-observation", selection:captured, version:copy, note:"正文摘要为 ZXS body 域摘要，不是 SHA-256；这份观察不是全系统恢复、最新性、在线权限或正式接纳证明。"}, null, 2)], {type:"application/json"}), `zxs-${copy.commit}.json`)); actions.append(receipt);
    }
    byId("copies").append(row);
  }
  byId("results").hidden = false;
}
byId("selection").addEventListener("input", resetResults);
byId("selection-file").addEventListener("change", async event => {
  resetResults(); const file = event.target.files[0]; if (!file) return;
  try { if (file.size > maxSelectionBytes) throw new Error("选择文件超过 60 KiB 上限。"); byId("selection").value = await file.text(); } catch (failure) { error(failure.message); }
});
byId("check-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !csrf) return;
  resetResults(); setBusy(true);
  try {
    const text = byId("selection").value;
    if (new TextEncoder().encode(text).length > maxSelectionBytes) throw new Error("选择 JSON 超过 60 KiB 上限。");
    const selection = JSON.parse(text);
    const response = await post("/api/authority/check", {selection, writers_stopped:byId("writers-stopped").checked});
    render(await response.json(), selection);
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("shutdown").addEventListener("click", async () => {
  if (!csrf || stopping || !window.confirm("结束此辅助工具？已开始的磁盘读取将等待完成后再退出。它不会停止 ZXS 或移除磁盘。")) return;
  error("");
  try { await post("/api/shutdown", {confirmation:"stop-helper"}); stopping = true; setBusy(busy); byId("shutdown").disabled = true; byId("connection").textContent = "已请求退出。请等启动终端显示进程真正结束，再拔盘或启动写入者。"; } catch (failure) { error(failure.message); }
});
setBusy(false); status().catch(failure => error(failure.message));
