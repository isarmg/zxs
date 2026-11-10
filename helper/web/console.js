"use strict";
const byId = id => document.getElementById(id);
let csrf = null;
let busy = false;
let stopping = false;
let installationPreview = null;
let programRepairPreview = null;

let legalMaterials = null;
let deploymentPreview = null;
let hostPreview = null;
let layoutPreview = null;
let runtimePreview = null;
let configurationPreview = null;
let activationPreview = null;
let activationOriginal = null;
let activationObserved = false;
const activationPrefix = "zxs-helper:service-activation:";
let configurationRecovery = null;
let administratorResetPreview = null;
let firstAdministratorPreview = null;
let firstStoragePreview = null;
let diskAdditionPreview = null;
let foreignChainPreview = null;
let foreignChainDatabasesPreview = null;
let foreignChainIdentityPreview = null;
let foreignChainObjectsPreview = null;
let foreignObjectsEpoch = 0;
let diskRemovalPreview = null;
let diskReadmissionPreview = null;
let chainTargetPlan = null;
let sambaPasswordPreview = null;
let sambaPasswordOriginal = null;
const maxSelectionBytes = 60 * 1024;
function error(message) { byId("error").textContent = message; byId("error").hidden = !message; }
function setBusy(value) { busy = value; for (const id of ["activation-preview","activation-start","activation-status","activation-retry","activation-retained","activation-enable-on-boot","activation-external-stopped"]) byId(id).disabled = value || stopping || !csrf; for (const id of ["foreign-chain-objects-preview","foreign-chain-objects-save"]) byId(id).disabled = value || stopping || !csrf; for (const input of byId("foreign-chain-objects-form").elements) input.disabled = value || stopping || !csrf; for (const id of ["foreign-chain-identity-preview","foreign-chain-identity-save"]) byId(id).disabled = value || stopping || !csrf; for (const input of byId("foreign-chain-databases-form").elements) input.disabled = value || stopping || !csrf; for (const id of ["foreign-chain-databases-preview","foreign-chain-databases-save"]) byId(id).disabled = value || stopping || !csrf; for (const form of ["configuration-recovery-form", "configuration-recovery-commit-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; byId("configuration-original-save").disabled = value || stopping || !csrf; for (const id of ["legal-materials-download", "legal-materials-manifest"]) byId(id).disabled = value || stopping || !csrf || !legalMaterials; for (const form of ["foreign-chain-form","foreign-chain-prepare-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; byId("foreign-chain-save").disabled = value || stopping || !csrf; for (const input of byId("program-repair-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("runtime-form").elements) input.disabled = value || stopping || !csrf; byId("runtime-prepare").disabled = value || stopping || !csrf; byId("runtime-external-stopped").disabled = value || stopping || !csrf; for (const input of byId("chain-generation-select-form").elements) input.disabled = value || stopping || !csrf; byId("chain-generation-read").disabled = value || stopping || !csrf; if (!chainTargetPlan) { byId("chain-generation-select-form").hidden = true; byId("chain-generation-read").hidden = true; } for (const input of byId("chain-generation-log-form").elements) input.disabled = value || stopping || !csrf; if (!chainTargetPlan) byId("chain-generation-log-form").hidden = true; for (const input of byId("chain-io-decision-form").elements) input.disabled = value || stopping || !csrf; if (!chainTargetPlan) byId("chain-io-decision-form").hidden = true; for (const input of byId("chain-epoch-form").elements) input.disabled = value || stopping || !csrf; if (!chainTargetPlan) byId("chain-epoch-form").hidden = true; byId("chain-current-read").disabled = value || stopping || !csrf; if (!chainTargetPlan) byId("chain-current-read").hidden = true; for (const input of byId("chain-publish-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("chain-first-form").elements) input.disabled = value || stopping || !csrf; byId("chain-target-status").disabled = value || stopping || !csrf; for (const input of byId("chain-target-form").elements) input.disabled = value || stopping || !csrf; byId("terminal-chain-readmit").disabled = value || stopping || !csrf; byId("terminal-chain-export").disabled = value || stopping || !csrf; for (const input of byId("disk-readmit-commit").elements) input.disabled = value || stopping || !csrf; for (const id of ["disk-readmit-preview","disk-readmit-original","disk-readmit-digest","disk-readmit-save"]) byId(id).disabled = value || stopping || !csrf; for (const input of byId("terminal-retire-commit").elements) input.disabled = value || stopping || !csrf; for (const id of ["terminal-retire-preview","terminal-retire-original","terminal-retire-digest","terminal-retire-save"]) byId(id).disabled = value || stopping || !csrf; for (const form of ["disk-remove-form","disk-remove-commit-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const form of ["disk-add-form","disk-add-commit-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const form of ["samba-reset-form","samba-password-form","samba-reset-status-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const form of ["first-storage-form", "first-storage-commit-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const input of byId("first-administrator-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("first-administrator-password-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("administrator-reset-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("administrator-password-form").elements) input.disabled = value || stopping || !csrf; byId("check").disabled = value || stopping || !csrf; byId("selection").disabled = value || stopping; byId("selection-file").disabled = value || stopping; byId("install-preview").disabled = value || stopping || !csrf; byId("install-commit").disabled = value || stopping || !csrf; for (const input of byId("deployment-form").elements) input.disabled = value || stopping || !csrf; byId("deployment-prepare").disabled = value || stopping || !csrf; byId("configuration-install").disabled = value || stopping || !csrf; byId("external-administration-stopped").disabled = value || stopping; byId("host-preview").disabled = value || stopping || !csrf; byId("host-prepare").disabled = value || stopping || !csrf; for (const input of byId("layout-form").elements) input.disabled = value || stopping || !csrf; byId("layout-prepare").disabled = value || stopping || !csrf; document.dispatchEvent(new Event("zxs-helper-busy-changed")); }
function resetResults() { byId("results").hidden = true; byId("copies").replaceChildren(); byId("report").textContent = ""; error(""); }
async function status() {
  const response = await fetch("/api/status", {cache:"no-store", credentials:"omit"});
  if (!response.ok) throw new Error(`无法连接辅助工具（HTTP ${response.status}）。请使用主机的 IP 和实际端口直接打开，不使用 DNS 名称或代理。`);
  const state = await response.json(); csrf = state.csrf; stopping = state.stopping;
  byId("connection").textContent = stopping ? "辅助工具正在等待已开始的读取结束并退出。" : `${state.name} ${state.version} · 独立维护模式 · 无登录 · ${state.busy ? "已有维护任务" : "没有进行中的维护任务"}`;
  legalMaterials = state.program_installation.legal_materials;
  byId("legal-materials-status").textContent = legalMaterials ? `内置许可材料 ${legalMaterials.archive_bytes} 字节 · 来源 ${legalMaterials.commit} · SHA-256 ${legalMaterials.archive_sha256}` : "此 helper 没有内置许可材料；不从网络或旁边文件补齐，请使用包含准确载荷与材料的正式 helper。";
  const embedded = state.program_installation.embedded;
  byId("payload-status").textContent = embedded ? `内置 ZXS ${embedded.version} · ${embedded.target} · ${embedded.bytes} 字节 · SHA-256 ${embedded.sha256}` : "当前为开发构建，没有内置安装载荷；不能下载替代载荷或假称可安装。请使用匹配版本的正式辅助工具。";
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

byId("install-preview").addEventListener("click", async () => {
  if (busy || stopping || !csrf) return;
  error(""); setBusy(true); installationPreview = null;
  byId("install-commit").hidden = true; byId("install-receipt").hidden = true;
  try {
    const response = await post("/api/install/preview", {});
    installationPreview = await response.json();
    byId("install-plan").textContent = JSON.stringify(installationPreview, null, 2);
    byId("install-plan").hidden = false; byId("install-commit").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("install-commit").addEventListener("click", async () => {
  if (busy || stopping || !installationPreview) return;
  const payload = installationPreview.plan.payload;
  if (!window.confirm(`仅安装 ZXS ${payload.version} 到 /usr/local/bin/zxs？\nSHA-256: ${payload.sha256}\n不会覆盖不同文件，也不会初始化、配置或启动服务。`)) return;
  error(""); setBusy(true);
  try {
    const response = await post("/api/install/commit", {request_digest:installationPreview.request_digest, confirmation:"install-program-only"});
    byId("install-receipt").textContent = JSON.stringify(await response.json(), null, 2);
    byId("install-receipt").hidden = false;
    installationPreview = null; byId("install-commit").hidden = true;
  } catch (failure) { error(`${failure.message}\n结果不确定时不要手动覆盖或删除临时材料；重新预览只会核对准确程序，不会替换不同字节。`); } finally { setBusy(false); }
});

function resetProgramRepair() {
  programRepairPreview = null;
  byId("program-repair-plan").hidden = true;
  byId("program-repair-receipt").hidden = true;
  byId("program-repair-commit").hidden = true;
  byId("program-repair-external-stopped").checked = false;
}
function showProgramRepair(preview) {
  programRepairPreview = {request_digest:preview.request_digest};
  byId("program-repair-digest").value = preview.request_digest;
  byId("program-repair-plan").textContent = JSON.stringify(preview, null, 2);
  byId("program-repair-plan").hidden = false;
  byId("program-repair-commit").hidden = false;
}
byId("program-repair-digest").addEventListener("input", resetProgramRepair);
byId("program-repair-preview").addEventListener("click", async () => {
  if (busy || stopping || !csrf) return;
  resetProgramRepair(); error(""); setBusy(true);
  try { showProgramRepair(await (await post("/api/install/repair-preview", {})).json()); }
  catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("program-repair-recover").addEventListener("click", async () => {
  if (busy || stopping || !csrf) return;
  const request_digest = byId("program-repair-digest").value;
  resetProgramRepair(); error(""); setBusy(true);
  try { showProgramRepair(await (await post("/api/install/repair-recover-preview", {request_digest})).json()); }
  catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("program-repair-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !csrf || !programRepairPreview) return;
  if (!byId("program-repair-external-stopped").checked) { error("先停止并确认外部 ZXS/Samba 和主机管理写者；host 租约不能替代它们的互斥。"); return; }
  if (!window.confirm("确认只恢复原安装基线的准确 ZXS 程序？已有原文件会独立备份并保留原 inode；不会改数据、配置、密码或启动服务。失败时保留全部证据，只用当前原摘要恢复。")) return;
  error(""); setBusy(true);
  try {
    const receipt = await (await post("/api/install/repair-commit", {...programRepairPreview,
      confirmation:"repair-same-payload-program-only", external_administration_stopped:true})).json();
    byId("program-repair-receipt").textContent = JSON.stringify(receipt, null, 2);
    byId("program-repair-receipt").hidden = false;
  } catch (failure) {
    error(`${failure.message}。失败不表示未替换，不能删除原备份或暂存程序。保留上面的摘要，重新连接后选择“读取原请求与当前状态”。`);
  } finally { setBusy(false); }
});

for (const [id, path, checksumKey, bytesKey, filename] of [
  ["legal-materials-download", "/api/install/materials-download", "archive_sha256", "archive_bytes", "zxs-license-materials.tar.gz"],
  ["legal-materials-manifest", "/api/install/materials-manifest", "manifest_sha256", "manifest_bytes", "zxs-license-materials.json"],
]) {
  byId(id).addEventListener("click", async () => {
    if (busy || stopping || !csrf || !legalMaterials) return;
    const expected = legalMaterials;
    error(""); setBusy(true);
    try {
      const response = await post(path, {});
      if (response.headers.get("x-zxs-material-sha256") !== expected[checksumKey]) throw new Error("内置材料响应与所选摘要不符，未保存。");
      const blob = await response.blob();
      if (blob.size !== expected[bytesKey]) throw new Error("内置材料响应长度不符，未保存。");
      save(blob, filename);
    } catch (failure) { error(failure.message); } finally { setBusy(false); }
  });
}


function resetActivationPreview() {
  activationPreview = null;
  byId("activation-plan").hidden = true;
  byId("activation-start").hidden = true;
  byId("activation-external-stopped").checked = false;
}
function activationInput() {
  return {deployment:{
    service_user:byId("service-user").value,
    service_group:byId("service-group").value,
    listen:byId("service-listen").value,
    smb:byId("smb-enabled").checked ? {
      account:byId("smb-account").value,peer_uid:Number(byId("smb-peer-uid").value)
    } : null
  },enable_on_boot:byId("activation-enable-on-boot").checked};
}
function activationRecord(record) {
  const input = record?.input;
  const settings = input?.deployment;
  if (!record || Object.keys(record).sort().join(",") !== "input,request_digest"
      || !/^[0-9a-f]{64}$/.test(record.request_digest)
      || !input || Object.keys(input).sort().join(",") !== "deployment,enable_on_boot"
      || typeof input.enable_on_boot !== "boolean" || !settings
      || Object.keys(settings).sort().join(",") !== "listen,service_group,service_user,smb"
      || typeof settings.service_user !== "string" || typeof settings.service_group !== "string"
      || typeof settings.listen !== "string"
      || (settings.smb !== null && (typeof settings.smb !== "object"
          || Object.keys(settings.smb).sort().join(",") !== "account,peer_uid"
          || typeof settings.smb.account !== "string"
          || !Number.isInteger(settings.smb.peer_uid) || settings.smb.peer_uid < 0
          || settings.smb.peer_uid > 4294967295))) {
    throw new Error("原启动请求格式不正确；保留原材料，不改写或自动重新开始。");
  }
  return record;
}
function refreshActivationRecords(selected = "") {
  const records = [];
  if (localStorage.length > 4096) throw new Error("浏览器存储键过多；未扫描或清理原请求。");
  for (let index = 0; index < localStorage.length; index++) {
    const key = localStorage.key(index);
    if (!key?.startsWith(activationPrefix)) continue;
    const text = localStorage.getItem(key);
    if (!text || text.length > 16 * 1024) throw new Error("原启动记录缺失或过大；未清理。");
    const record = activationRecord(JSON.parse(text));
    if (key !== activationPrefix + record.request_digest) throw new Error("原启动记录与摘要键不符；未清理。");
    records.push(record);
    if (records.length > 16) throw new Error("保留的启动请求超过 16 条；先保全原材料，不能静默丢弃。");
  }
  const select = byId("activation-retained");
  select.replaceChildren();
  const placeholder = document.createElement("option");
  placeholder.value = ""; placeholder.textContent = records.length ? "选择要查询的原请求" : "没有已保留请求";
  select.append(placeholder);
  for (const record of records) {
    const option = document.createElement("option");
    option.value = record.request_digest;
    option.textContent = record.request_digest + " · " + record.input.deployment.listen;
    select.append(option);
  }
  select.value = selected || (records.length === 1 ? records[0].request_digest : "");
  activationOriginal = records.find(record => record.request_digest === select.value) || null;
  activationObserved = false;
  byId("activation-status").hidden = !activationOriginal;
  byId("activation-retry").hidden = true;
  return records.length;
}
function showActivationResult(result, original) {
  if (result.request_digest !== original.request_digest) throw new Error("响应不属于这个原启动请求，结果仍未知。");
  byId("activation-result").textContent = JSON.stringify(result,null,2);
  byId("activation-result").hidden = false;
  const ready = result.state === "services_active_and_management_ready";
  byId("activation-open").hidden = !ready;
  if (ready) {
    const url = new URL(window.location.href);
    url.protocol = "http:";
    const listen = original.input.deployment.listen;
    const port = listen.slice(listen.lastIndexOf(":") + 1);
    if (!/^[0-9]+$/.test(port) || Number(port) < 1 || Number(port) > 65535) throw new Error("原服务端口无效。");
    url.port = port; url.pathname = "/"; url.search = ""; url.hash = "";
    byId("activation-web").href = url.toString();
  }
}
async function sendActivation(original) {
  if (!byId("activation-external-stopped").checked) {
    error("请先停止其他主机管理操作，并明确勾选服务动作确认。"); return;
  }
  const smb = original.input.deployment.smb ? "以及独立 zxs-smb.service" : "（不启动 Samba）";
  if (!window.confirm(`以 root 执行原请求 ${original.request_digest} 的 daemon-reload 和启动 zxs.service ${smb}？${original.input.enable_on_boot ? "就绪后还将启用这些服务的开机启动。" : "不会更改开机启动设置。"} 不停止其他服务、不初始化数据或重置密码。`)) return;
  const text = JSON.stringify(activationRecord(original));
  try {
    const count = refreshActivationRecords(original.request_digest);
    const key = activationPrefix + original.request_digest;
    const retained = localStorage.getItem(key);
    if (text.length > 16 * 1024 || (!retained && count >= 16)) throw new Error("原请求保存预算已满");
    if (retained && retained !== text) throw new Error("同摘要已有不同原请求，未覆盖");
    // Per-digest immutable keys preserve other tabs' different requests.
    localStorage.setItem(key,text);
    if (localStorage.getItem(key) !== text) throw new Error("原启动请求未能准确读回");
    refreshActivationRecords(original.request_digest);
  } catch (failure) {
    error(`尚未发送启动请求：${failure.message}。`); return;
  }
  activationOriginal = original;
  activationObserved = false;
  resetActivationPreview();
  byId("activation-open").hidden = true;
  setBusy(true); error("");
  try {
    const result = await (await post("/api/deployment/activate",{
      ...original,confirmation:"activate-installed-zxs-services",external_administration_stopped:true
    })).json();
    showActivationResult(result,original);
  } catch (failure) {
    error(`${failure.message} 原请求 ${original.request_digest} 的结果可能已生效；只查询后再明确决定是否重试。`);
  } finally {
    byId("activation-status").hidden = false;
    byId("activation-retry").hidden = true;
    setBusy(false);
  }
}
byId("activation-enable-on-boot").addEventListener("input",resetActivationPreview);
byId("activation-preview").addEventListener("click",async () => {
  if (busy || stopping || !csrf) return;
  if (!byId("deployment-form").reportValidity()) return;
  const input = activationInput();
  resetActivationPreview(); setBusy(true); error("");
  try {
    const preview = await (await post("/api/deployment/activation-preview",input)).json();
    activationPreview = activationRecord({input,request_digest:preview.request_digest});
    byId("activation-plan").textContent = JSON.stringify(preview,null,2);
    byId("activation-plan").hidden = false;
    byId("activation-start").hidden = false;
  } catch (failure) { error(failure.message); }
  finally { setBusy(false); }
});
byId("activation-start").addEventListener("click",async () => {
  if (busy || stopping || !csrf || !activationPreview) return;
  await sendActivation(activationPreview);
});
byId("activation-retained").addEventListener("change",() => {
  try {
    refreshActivationRecords(byId("activation-retained").value);
    byId("activation-open").hidden = true;
    if (activationOriginal) {
      byId("activation-result").textContent = JSON.stringify({original_request:activationOriginal,state:"query_before_retry"},null,2);
      byId("activation-result").hidden = false;
    }
  } catch (failure) {
    activationOriginal = null; activationObserved = false;
    byId("activation-status").hidden = true; byId("activation-retry").hidden = true;
    error(failure.message);
  }
});
byId("activation-status").addEventListener("click",async () => {
  if (busy || stopping || !csrf || !activationOriginal) return;
  const original = activationOriginal;
  activationObserved = false; byId("activation-retry").hidden = true;
  byId("activation-open").hidden = true; setBusy(true); error("");
  try {
    const result = await (await post("/api/deployment/activation-status",original)).json();
    showActivationResult(result,original);
    activationObserved = true;
    byId("activation-retry").hidden = result.state === "services_active_and_management_ready";
  } catch (failure) {
    error(`${failure.message} 未启动、停止或禁用任何服务；原结果仍需核对，不能直接按新设置重发。`);
  } finally { setBusy(false); }
});
byId("activation-retry").addEventListener("click",async () => {
  if (busy || stopping || !csrf || !activationOriginal || !activationObserved) return;
  await sendActivation(activationOriginal);
});
try { refreshActivationRecords(); }
catch (failure) { error(failure.message); }

function resetDeployment() {
  resetActivationPreview();
  resetConfiguration();
  deploymentPreview = null; byId("deployment-prepare").hidden = true;
  byId("deployment-plan").hidden = true; byId("deployment-receipt").hidden = true;
}
byId("deployment-form").addEventListener("input", resetDeployment);
byId("smb-enabled").addEventListener("change", () => {
  const enabled = byId("smb-enabled").checked;
  byId("smb-fields").hidden = !enabled;
  byId("smb-account").required = enabled; byId("smb-peer-uid").required = enabled;
  resetDeployment();
});
byId("deployment-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping) return;
  const input = {service_user:byId("service-user").value,service_group:byId("service-group").value,
    listen:byId("service-listen").value,smb:byId("smb-enabled").checked ? {
      account:byId("smb-account").value,peer_uid:Number(byId("smb-peer-uid").value)} : null};
  resetDeployment(); error(""); setBusy(true);
  try {
    const preview = await (await post("/api/deployment/preview", input)).json();
    deploymentPreview = {input,request_digest:preview.request_digest};
    byId("deployment-plan").textContent = JSON.stringify(preview, null, 2);
    byId("deployment-plan").hidden = false; byId("deployment-prepare").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("deployment-prepare").addEventListener("click", async () => {
  if (busy || stopping || !deploymentPreview) return;
  if (!window.confirm("确认仅将预览中的未激活配置保存到 /zxs 下的新专用目录？不会安装或启用服务，也不会修改当前 Samba 配置和密码。已有不同内容不会被覆盖。")) return;
  error(""); setBusy(true);
  try {
    const receipt = await (await post("/api/deployment/prepare", {...deploymentPreview,confirmation:"prepare-inactive-configuration"})).json();
    byId("deployment-receipt").textContent = JSON.stringify(receipt, null, 2);
    byId("deployment-receipt").hidden = false;
  } catch (failure) { error(`${failure.message}。如果连接中断，不要删除 pending 材料；保留准确预览并重试同一请求。`); }
  finally { setBusy(false); }
});

byId("host-preview").addEventListener("click", async () => {
  if (busy || stopping) return;
  hostPreview = null; byId("host-prepare").hidden = true; byId("host-receipt").hidden = true;
  byId("host-plan").hidden = true; error(""); setBusy(true);
  try {
    hostPreview = await (await post("/api/host/preview", {})).json();
    byId("host-plan").textContent = JSON.stringify(hostPreview, null, 2);
    byId("host-plan").hidden = false; byId("host-prepare").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("host-prepare").addEventListener("click", async () => {
  if (busy || stopping || !hostPreview) return;
  if (!window.confirm("确认只准备 /zxs 安装根？已有安全目录不改权限和内容；不会创建账户、初始化磁盘或启动服务。这不是系统初始化完成。")) return;
  error(""); setBusy(true);
  try {
    const receipt = await (await post("/api/host/prepare", {request_digest:hostPreview.request_digest,confirmation:"prepare-host-root-only"})).json();
    byId("host-receipt").textContent = JSON.stringify(receipt, null, 2); byId("host-receipt").hidden = false;
  } catch (failure) { error(`${failure.message}。结果不确定时保留 pending 材料，仅按准确请求重试，不手工清空目录。`); }
  finally { setBusy(false); }
});

function resetLayout() {
  layoutPreview = null; byId("layout-plan").hidden = true;
  byId("layout-prepare").hidden = true; byId("layout-receipt").hidden = true;
}
byId("layout-user").addEventListener("input", resetLayout);
byId("layout-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping) return;
  const input = {service_user:byId("layout-user").value};
  resetLayout(); error(""); setBusy(true);
  try {
    const preview = await (await post("/api/initialization/layout-preview", input)).json();
    layoutPreview = {input,request_digest:preview.request_digest};
    byId("layout-plan").textContent = JSON.stringify(preview, null, 2);
    byId("layout-plan").hidden = false; byId("layout-prepare").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("layout-prepare").addEventListener("click", async () => {
  if (busy || stopping || !layoutPreview) return;
  if (!window.confirm("确认只将 /zxs/data 的新空目录树交给预览中的本机服务 UID/GID？不会改现有目录权限，不创建管理员、数据库或磁盘。先确保旧版与其他写者已停止。")) return;
  error(""); setBusy(true);
  try {
    const receipt = await (await post("/api/initialization/layout-prepare", {...layoutPreview,confirmation:"prepare-private-layout-only"})).json();
    byId("layout-receipt").textContent = JSON.stringify(receipt, null, 2); byId("layout-receipt").hidden = false;
  } catch (failure) { error(`${failure.message}。保留 pending 目录；失败不证明未写入，不能以重新初始化或清空目录代替恢复。`); }
  finally { setBusy(false); }
});

function resetRuntime() {
  runtimePreview = null;
  byId("runtime-plan").hidden = true;
  byId("runtime-receipt").hidden = true;
  byId("runtime-prepare").hidden = true;
  byId("runtime-external-stopped").checked = false;
}
byId("runtime-user").addEventListener("input", resetRuntime);
byId("runtime-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !csrf) return;
  const input = {service_user:byId("runtime-user").value};
  resetRuntime(); error(""); setBusy(true);
  try {
    const preview = await (await post("/api/initialization/runtime-preview", input)).json();
    runtimePreview = {input, request_digest:preview.request_digest};
    byId("runtime-plan").textContent = JSON.stringify(preview, null, 2);
    byId("runtime-plan").hidden = false;
    byId("runtime-prepare").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("runtime-prepare").addEventListener("click", async () => {
  if (busy || stopping || !csrf || !runtimePreview) return;
  if (!byId("runtime-external-stopped").checked) {
    error("请先停止外部 ZXS/Samba 与主机配置写者，再确认。辅助工具的 host 租约不能替代它们的互斥。"); return;
  }
  if (!window.confirm("只准备预览中的 6 个服务私有空运行目录和 root 所有的空 SMB 导出目录？不会清理已有文件、修改现有目录权限或启动服务。部分发布失败时保留原意图和 pending，仅续作原请求。")) return;
  error(""); setBusy(true);
  try {
    const receipt = await (await post("/api/initialization/runtime-prepare", {
      ...runtimePreview, confirmation:"prepare-empty-runtime-only", external_administration_stopped:true,
    })).json();
    byId("runtime-receipt").textContent = JSON.stringify(receipt, null, 2);
    byId("runtime-receipt").hidden = false;
  } catch (failure) {
    error(`${failure.message}。不确定结果不是未写入；保留原请求和全部目录。已有完成回执与当前目录身份不符时会拒绝重建，不要删除证据。`);
  } finally { setBusy(false); }
});

function resetConfiguration() {
  configurationPreview = null;
  resetConfigurationRecovery();
  byId("configuration-plan").hidden = true;
  byId("configuration-install").hidden = true;
  byId("configuration-receipt").hidden = true;
  byId("external-administration-stopped").checked = false;
}
byId("configuration-preview").addEventListener("click", async () => {
  if (busy || stopping || !byId("deployment-form").reportValidity()) return;
  const input = {service_user:byId("service-user").value,service_group:byId("service-group").value,
    listen:byId("service-listen").value,smb:byId("smb-enabled").checked ? {
      account:byId("smb-account").value,peer_uid:Number(byId("smb-peer-uid").value)} : null};
  resetConfiguration(); error(""); setBusy(true);
  try {
    const preview = await (await post("/api/deployment/install-preview", input)).json();
    configurationPreview = {input,request_digest:preview.request_digest};
    byId("configuration-recovery-digest").value = preview.request_digest;
    byId("configuration-original-save").hidden = false;
    byId("configuration-plan").textContent = JSON.stringify(preview, null, 2);
    byId("configuration-plan").hidden = false;
    byId("configuration-install").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("configuration-install").addEventListener("click", async () => {
  if (busy || stopping || !configurationPreview) return;
  if (!byId("external-administration-stopped").checked) {
    error("请先停止外部配置/服务管理操作并明确确认。当前 ZXS 租约不等于已停止所有外部写入者。"); return;
  }
  if (!window.confirm("确认仅安装预览中的 /etc/systemd/system/zxs.service 和可选 /etc/samba/zxs.conf？不覆盖不同文件，不更改默认 smb.conf 或密码，不加载/启动/启用服务。本步骤不启动服务；初始化与依赖完备后可使用下方的独立显式启动入口。")) return;
  error(""); setBusy(true);
  try {
    const receipt = await (await post("/api/deployment/install-files", {...configurationPreview,
      confirmation:"install-configuration-files-only",external_administration_stopped:true})).json();
    byId("configuration-receipt").textContent = JSON.stringify(receipt, null, 2);
    byId("configuration-receipt").hidden = false;
  } catch (failure) {
    error(`${failure.message}。跨目录安装不是一个原子事务，已有文件可能已经发布。保留准确输入/摘要、intent/installed 记录和临时文件，只重试同一操作，不删证据或覆盖未知配置。`);
  } finally { setBusy(false); }
});

function resetConfigurationRecovery() {
  configurationRecovery = null;
  byId("configuration-recovery-plan").hidden = true;
  byId("configuration-recovery-receipt").hidden = true;
  byId("configuration-recovery-commit-form").hidden = true;
  byId("configuration-original-save").hidden = true;
  byId("configuration-recovery-external-stopped").checked = false;
}
byId("configuration-recovery-digest").addEventListener("input", resetConfigurationRecovery);
byId("configuration-recovery-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !csrf) return;
  const request_digest = byId("configuration-recovery-digest").value;
  resetConfigurationRecovery(); error(""); setBusy(true);
  try {
    const preview = await (await post("/api/deployment/install-recover-preview", {request_digest})).json();
    configurationRecovery = {request_digest:preview.request_digest};
    byId("configuration-recovery-plan").textContent = JSON.stringify(preview, null, 2);
    byId("configuration-recovery-plan").hidden = false;
    byId("configuration-recovery-commit-form").hidden = !preview.continuation_available;
    byId("configuration-original-save").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("configuration-original-save").addEventListener("click", () => {
  if (busy || stopping || !csrf) return;
  const reference = {format:"zxs-helper-configuration-request-reference", revision:2,
    request_digest:byId("configuration-recovery-digest").value};
  save(new Blob([JSON.stringify(reference, null, 2)], {type:"application/json"}), "zxs-configuration-original-request.json");
});
byId("configuration-recovery-commit-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !csrf || !configurationRecovery) return;
  if (!byId("configuration-recovery-external-stopped").checked) { error("请先停止外部配置/服务管理者并确认；host 租约不是所有外部写者的排他证明。"); return; }
  if (!window.confirm("只继续已读取的原配置安装？不会用当前表单生成新请求，不覆盖已有不同配置，不启动/启用服务；完成后缺件或原记录不完整将保留拒绝。")) return;
  error(""); setBusy(true);
  try {
    const result = await (await post("/api/deployment/install-recover-files", {...configurationRecovery,
      confirmation:"resume-original-configuration-files-only", external_administration_stopped:true})).json();
    byId("configuration-recovery-receipt").textContent = JSON.stringify(result, null, 2);
    byId("configuration-recovery-receipt").hidden = false;
    byId("configuration-recovery-commit-form").hidden = true;
    configurationRecovery = null;
  } catch (failure) {
    error(`${failure.message}。结果不确定时保留原摘要、全部 request/intent/installed 与暂存文件；重新读取原操作，不删记录或重建已完成后丢失的配置。`);
  } finally { setBusy(false); }
});

function clearAdministratorPasswords() {
  byId("administrator-new-password").value = "";
  byId("administrator-repeat-password").value = "";
}
function resetAdministratorPlan() {
  byId("administrator-reset-save-plan").hidden = true;
  administratorResetPreview = null; clearAdministratorPasswords();
  byId("administrator-reset-plan").hidden = true;
  byId("administrator-password-form").hidden = true;
  byId("administrator-reset-receipt").hidden = true;
  byId("credential-writers-stopped").checked = false;
}
byId("reset-service-user").addEventListener("input", resetAdministratorPlan);
byId("administrator-reset-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping) return;
  const input = {service_user:byId("reset-service-user").value};
  resetAdministratorPlan(); error(""); setBusy(true);
  try {
    const preview = await (await post("/api/credentials/reset-preview", input)).json();
    administratorResetPreview = {input,request_digest:preview.request_digest};
    byId("administrator-reset-plan").textContent = JSON.stringify(preview, null, 2);
    byId("administrator-reset-plan").hidden = false;
    byId("administrator-password-form").hidden = false;
    byId("administrator-reset-save-plan").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("administrator-password-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !administratorResetPreview) return;
  if (byId("administrator-new-password").value !== byId("administrator-repeat-password").value) {
    error("两次输入的新密码不一致。"); return;
  }
  if (!window.confirm("确认仅重置预览中的已存在 ZXS 管理员密码并撤销全部 ZXS 会话？不更改 Samba 密码，不初始化系统，灾备旧凭据副本不会在此更新。")) return;
  const body = {...administratorResetPreview,confirmation:"reset-existing-zxs-administrator-password",
    external_writers_stopped:byId("credential-writers-stopped").checked,
    new_password:byId("administrator-new-password").value,
    repeat_password:byId("administrator-repeat-password").value};
  clearAdministratorPasswords(); error(""); setBusy(true);
  try {
    const response = await post("/api/credentials/reset", body);
    const receipt = await response.json();
    byId("administrator-reset-receipt").textContent = JSON.stringify(receipt, null, 2);
    byId("administrator-reset-receipt").hidden = false;
    administratorResetPreview = null;
    byId("administrator-password-form").hidden = true;
    byId("administrator-reset-save-plan").hidden = true;
  } catch (failure) {
    error(`${failure.message}。密码可能已经改变；保留原计划/摘要，再次输入同一个新密码续作，不以失败响应推断旧密码仍有效。`);
  } finally {
    body.new_password = ""; body.repeat_password = "";
    clearAdministratorPasswords(); setBusy(false);
  }
});
window.addEventListener("pagehide", clearAdministratorPasswords);

byId("reset-original-digest").addEventListener("input", resetAdministratorPlan);
byId("administrator-reset-save-plan").addEventListener("click", () => {
  if (!administratorResetPreview) return;
  save(new Blob([JSON.stringify(administratorResetPreview, null, 2)], {type:"application/json"}),
    `zxs-administrator-reset-${administratorResetPreview.request_digest}.json`);
});
byId("administrator-reset-recover").addEventListener("click", async () => {
  if (busy || stopping) return;
  const request_digest = byId("reset-original-digest").value;
  if (!/^[0-9a-f]{64}$/.test(request_digest)) { error("请输入原操作的完整 64 位小写摘要。"); return; }
  const input = {service_user:byId("reset-service-user").value};
  resetAdministratorPlan(); error(""); setBusy(true);
  try {
    const preview = await (await post("/api/credentials/reset-recover-preview", {input,request_digest})).json();
    administratorResetPreview = {input,request_digest:preview.request_digest};
    byId("administrator-reset-plan").textContent = JSON.stringify(preview, null, 2);
    byId("administrator-reset-plan").hidden = false;
    byId("administrator-password-form").hidden = false;
    byId("administrator-reset-save-plan").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});

function clearFirstAdministratorPasswords() {
  byId("first-administrator-password").value = "";
  byId("first-administrator-repeat-password").value = "";
}
function resetFirstAdministratorPlan() {
  firstAdministratorPreview = null; clearFirstAdministratorPasswords();
  byId("first-administrator-plan").hidden = true;
  byId("first-administrator-save-plan").hidden = true;
  byId("first-administrator-password-form").hidden = true;
  byId("first-administrator-receipt").hidden = true;
  byId("first-writers-stopped").checked = false;
}
function firstAdministratorInput() {
  return {service_user:byId("first-service-user").value,administrator:byId("first-administrator-name").value};
}
function showFirstAdministratorPlan(input, preview) {
  firstAdministratorPreview = {input,request_digest:preview.request_digest};
  byId("first-administrator-plan").textContent = JSON.stringify(preview, null, 2);
  byId("first-administrator-plan").hidden = false;
  byId("first-administrator-save-plan").hidden = false;
  byId("first-administrator-password-form").hidden = false;
}
for (const id of ["first-service-user","first-administrator-name","first-original-digest"]) {
  byId(id).addEventListener("input", resetFirstAdministratorPlan);
}
byId("first-administrator-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping) return;
  const input = firstAdministratorInput(); resetFirstAdministratorPlan(); error(""); setBusy(true);
  try { showFirstAdministratorPlan(input, await (await post("/api/initialization/administrator-preview", input)).json()); }
  catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("first-administrator-recover").addEventListener("click", async () => {
  if (busy || stopping) return;
  const request_digest = byId("first-original-digest").value;
  if (!/^[0-9a-f]{64}$/.test(request_digest)) { error("请输入原初始化请求的完整摘要。"); return; }
  const input = firstAdministratorInput(); resetFirstAdministratorPlan(); error(""); setBusy(true);
  try { showFirstAdministratorPlan(input, await (await post("/api/initialization/administrator-recover-preview", {input,request_digest})).json()); }
  catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("first-administrator-save-plan").addEventListener("click", () => {
  if (!firstAdministratorPreview) return;
  save(new Blob([JSON.stringify(firstAdministratorPreview, null, 2)], {type:"application/json"}),
    `zxs-first-administrator-${firstAdministratorPreview.request_digest}.json`);
});
byId("first-administrator-password-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !firstAdministratorPreview) return;
  if (byId("first-administrator-password").value !== byId("first-administrator-repeat-password").value) { error("两次初始密码不一致。"); return; }
  if (!window.confirm("确认只在准确新空布局中创建所示 ZXS 管理员？不创建 Unix/Samba 账户，不初始化磁盘，不表示完整系统或存储服务已就绪。失败时只按原请求和同一密码续作，不清空未知数据。")) return;
  const body = {...firstAdministratorPreview,confirmation:"initialize-first-management-administrator",
    external_writers_stopped:byId("first-writers-stopped").checked,password:byId("first-administrator-password").value,
    repeat_password:byId("first-administrator-repeat-password").value};
  clearFirstAdministratorPasswords(); error(""); setBusy(true);
  try {
    const receipt = await (await post("/api/initialization/administrator-create", body)).json();
    byId("first-administrator-receipt").textContent = JSON.stringify(receipt, null, 2);
    byId("first-administrator-receipt").hidden = false;
    firstAdministratorPreview = null;
    byId("first-administrator-password-form").hidden = true;
    byId("first-administrator-save-plan").hidden = true;
  } catch (failure) { error(`${failure.message}。管理员可能已创建；保留原请求，使用相同密码续作，不删除记录或改成新的初始化。`); }
  finally { body.password = ""; body.repeat_password = ""; clearFirstAdministratorPasswords(); setBusy(false); }
});
window.addEventListener("pagehide", clearFirstAdministratorPasswords);

function resetFirstStoragePlan() {
  firstStoragePreview = null;
  for (const id of ["first-storage-plan","first-storage-save","first-storage-commit-form","first-storage-receipt"]) byId(id).hidden = true;
  byId("first-storage-writers").checked = false;
}
function showFirstStoragePlan(service_user, preview) {
  firstStoragePreview = {input:{service_user,request_digest:preview.request_digest},plan:preview.plan};
  byId("first-storage-plan").textContent = JSON.stringify(preview,null,2);
  for (const id of ["first-storage-plan","first-storage-save","first-storage-commit-form"]) byId(id).hidden = false;
}
for (const id of ["first-storage-user","first-storage-roots","first-storage-digest"]) byId(id).addEventListener("input",resetFirstStoragePlan);
byId("first-storage-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping) return;
  const service_user = byId("first-storage-user").value;
  const roots = byId("first-storage-roots").value.split(/\r?\n/).filter(line => line.length > 0);
  resetFirstStoragePlan(); error(""); setBusy(true);
  try { showFirstStoragePlan(service_user, await (await post("/api/initialization/storage-preview", {service_user,roots})).json()); }
  catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("first-storage-recover").addEventListener("click", async () => {
  if (busy || stopping) return;
  const service_user = byId("first-storage-user").value;
  const request_digest = byId("first-storage-digest").value;
  if (!/^[0-9a-f]{64}$/.test(request_digest)) { error("请输入原首批存储请求的完整摘要。"); return; }
  resetFirstStoragePlan(); error(""); setBusy(true);
  try { showFirstStoragePlan(service_user, await (await post("/api/initialization/storage-recover-preview", {service_user,request_digest})).json()); }
  catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("first-storage-save").addEventListener("click", () => {
  if (!firstStoragePreview) return;
  save(new Blob([JSON.stringify(firstStoragePreview,null,2)],{type:"application/json"}),`zxs-first-storage-${firstStoragePreview.input.request_digest}.json`);
});
byId("first-storage-commit-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !firstStoragePreview) return;
  const roots = firstStoragePreview.plan.selections.map(selection => selection.root_path);
  if (!window.confirm(`确认把以下全部现有空目录初始化为新的生产池独立一级盘？\n${roots.join("\n")}\n不格式化、不启动服务；失败后保留证据并只续作原请求。`)) return;
  const body = {...firstStoragePreview,confirmation:"initialize-first-storage-batch",external_writers_stopped:byId("first-storage-writers").checked};
  error(""); setBusy(true);
  try {
    const receipt = await (await post("/api/initialization/storage-create",body)).json();
    byId("first-storage-receipt").textContent = JSON.stringify(receipt,null,2);
    byId("first-storage-receipt").hidden = false;
    firstStoragePreview = null;
    byId("first-storage-commit-form").hidden = true;
    byId("first-storage-save").hidden = true;
  } catch (failure) { error(`${failure.message}。存储可能已部分或全部初始化。保留原计划及摘要，不删除磁盘或控制文件、不换请求清盘重做。`); }
  finally { setBusy(false); }
});

function sambaDeploymentInput() {
  if (!byId("smb-enabled").checked) throw new Error("请先启用上方配置中的 Samba 选项，并填写与已安装文件相同的设置。");
  return {service_user:byId("service-user").value,service_group:byId("service-group").value,
    listen:byId("service-listen").value,smb:{account:byId("smb-account").value,peer_uid:Number(byId("smb-peer-uid").value)}};
}
function clearSambaPasswords() {
  byId("samba-new-password").value = ""; byId("samba-repeat-password").value = "";
}
function invalidateSambaPreview() {
  sambaPasswordPreview = null; clearSambaPasswords();
  byId("samba-password-form").hidden = true; byId("samba-reset-plan").hidden = true;
}
function sambaAction(kind) {
  if (kind === "create_missing") return {
    preview:"/api/credentials/samba-account-preview", commit:"/api/credentials/samba-account-create",
    status:"/api/credentials/samba-account-status", confirmation:"create-missing-local-samba-account",
    title:"首次登记缺失的本地 Samba 账户"
  };
  if (kind === "reset_existing") return {
    preview:"/api/credentials/samba-preview", commit:"/api/credentials/samba-reset",
    status:"/api/credentials/samba-status", confirmation:"reset-existing-local-samba-password",
    title:"重置已存在的本地 Samba 密码"
  };
  throw new Error("请选择明确的 Samba 操作类型。");
}
const sambaOriginalKey = "zxs-helper-samba-original:";
function preserveSambaOriginal(original) {
  const text = JSON.stringify(original);
  const key = sambaOriginalKey + original.kind + ":" + original.request_digest;
  const previous = localStorage.getItem(key);
  if (previous !== null && previous !== text) throw new Error("本机保存的原 Samba 请求不同，未发送命令；请保留原记录。");
  localStorage.setItem(key,text);
  localStorage.setItem(sambaOriginalKey + "last",text);
}
function restoreSambaOriginal() {
  const text = localStorage.getItem(sambaOriginalKey + "last");
  if (text === null) return;
  const original = JSON.parse(text);
  sambaAction(original.kind);
  if (!/^[0-9a-f]{64}$/.test(original.request_digest) || !original.input ||
      !/^[0-9a-f]{32}$/.test(original.input.operation_id)) throw new Error("本机 Samba 原请求记录无效，未重放操作。");
  sambaPasswordOriginal = original;
  byId("samba-status-kind").value = original.kind;
  byId("samba-reset-digest").value = original.request_digest;
  byId("samba-reset-save").hidden = false;
  byId("samba-reset-receipt").textContent = "已恢复无密码的原操作索引。请显式查询；刷新不会提交或重放密码命令。";
  byId("samba-reset-receipt").hidden = false;
}
byId("deployment-form").addEventListener("input",invalidateSambaPreview);
byId("samba-reset-form").addEventListener("input",invalidateSambaPreview);
byId("samba-reset-form").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping) return;
  invalidateSambaPreview(); error("");
  let input, action, kind;
  try {
    kind = byId("samba-operation-kind").value; action = sambaAction(kind);
    const nonce = crypto.getRandomValues(new Uint8Array(16));
    const operation_id = Array.from(nonce,byte => byte.toString(16).padStart(2,"0")).join("");
    input = {deployment:sambaDeploymentInput(),operation_id,external_writers_stopped:byId("samba-writers-stopped").checked};
  } catch (failure) { error(failure.message); return; }
  setBusy(true);
  try {
    const preview = await (await post(action.preview,input)).json();
    sambaPasswordPreview = {kind,input,request_digest:preview.request_digest};
    sambaPasswordOriginal = sambaPasswordPreview;
    byId("samba-status-kind").value = kind;
    byId("samba-reset-digest").value = preview.request_digest;
    byId("samba-reset-plan").textContent = JSON.stringify(preview,null,2);
    byId("samba-password-submit").textContent = "确认" + action.title;
    for (const id of ["samba-reset-plan","samba-reset-save","samba-password-form"]) byId(id).hidden = false;
    byId("samba-reset-receipt").hidden = true;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("samba-reset-save").addEventListener("click",() => {
  if (!sambaPasswordOriginal) return;
  save(new Blob([JSON.stringify(sambaPasswordOriginal,null,2)],{type:"application/json"}),`zxs-samba-${sambaPasswordOriginal.kind}-${sambaPasswordOriginal.request_digest}.json`);
});
byId("samba-password-form").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping || !sambaPasswordPreview) return;
  if (byId("samba-new-password").value !== byId("samba-repeat-password").value) { error("两次 Samba 新密码不一致。"); return; }
  const original = sambaPasswordPreview;
  const action = sambaAction(original.kind);
  const name = original.input.deployment.smb.account;
  const effect = original.kind === "create_missing"
    ? "只准已核缺失的名称及 UID；将新建原生 Samba 账户，并可能初始化本机 passdb/身份。原生 -a 不是原子只创建命令，外部写入者必须持续停止，否则并发出现的账户可能被改密。"
    : "只重置已存在账户，不创建或主动启用账户。";
  if (!window.confirm(`确认${action.title} ${name}？${effect}不更改 ZXS/Unix 密码，不启动服务。结果不明时只查询原请求，不自动重放。`)) return;
  try { preserveSambaOriginal(original); }
  catch (failure) { clearSambaPasswords(); error(`无法保留无密码原请求，命令未发送：${failure.message}`); return; }
  const body = {input:original.input,request_digest:original.request_digest,confirmation:action.confirmation,
    new_password:byId("samba-new-password").value,repeat_password:byId("samba-repeat-password").value};
  sambaPasswordPreview = null; clearSambaPasswords(); byId("samba-password-form").hidden = true;
  error(""); setBusy(true);
  try {
    const receipt = await (await post(action.commit,body)).json();
    byId("samba-reset-receipt").textContent = JSON.stringify(receipt,null,2);
    byId("samba-reset-receipt").hidden = false;
  } catch (failure) { error(`${failure.message}。账户或密码可能已改变。保留原摘要和操作类型并查询结果，不重复提交原操作，也不要恢复旧 passdb 覆盖其他账户。`); }
  finally { body.new_password = ""; body.repeat_password = ""; clearSambaPasswords(); setBusy(false); }
});
byId("samba-reset-status-form").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping) return;
  const request_digest = byId("samba-reset-digest").value;
  if (!/^[0-9a-f]{64}$/.test(request_digest)) { error("请输入原 Samba 操作的完整摘要。"); return; }
  let action;
  try { action = sambaAction(byId("samba-status-kind").value); }
  catch (failure) { error(failure.message); return; }
  error(""); setBusy(true);
  try {
    const result = await (await post(action.status,{request_digest})).json();
    byId("samba-reset-receipt").textContent = JSON.stringify(result,null,2); byId("samba-reset-receipt").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
window.addEventListener("pagehide",clearSambaPasswords);
try { restoreSambaOriginal(); } catch (failure) { error(failure.message); }

function resetDiskAddition() {
  diskAdditionPreview = null;
  for (const id of ["disk-add-plan","disk-add-save","disk-add-commit-form","disk-add-result"]) byId(id).hidden = true;
  byId("disk-add-confirm").checked = false;
}
function showDiskAddition(service_user, preview) {
  diskAdditionPreview = {input:{service_user,request_digest:preview.request_digest,external_writers_stopped:true},plan:preview.plan};
  byId("disk-add-digest").value = preview.request_digest;
  byId("disk-add-plan").textContent = JSON.stringify(preview,null,2);
  for (const id of ["disk-add-plan","disk-add-save","disk-add-commit-form"]) byId(id).hidden = false;
}
for (const id of ["disk-add-user","disk-add-input","disk-add-digest","disk-add-writers"]) byId(id).addEventListener("input",resetDiskAddition);
byId("disk-add-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping) return;
  resetDiskAddition(); error("");
  const service_user = byId("disk-add-user").value;
  let additions;
  try { additions = JSON.parse(byId("disk-add-input").value); }
  catch { error("新目录必须是明确列出整批根目录和关系的 JSON 数组。"); return; }
  const operation_id = "op_" + Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2,"0")).join("");
  setBusy(true);
  try { showDiskAddition(service_user, await (await post("/api/disks/add-preview",{service_user,operation_id,additions,external_writers_stopped:byId("disk-add-writers").checked})).json()); }
  catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("disk-add-original").addEventListener("click", async () => {
  if (busy || stopping) return;
  const service_user = byId("disk-add-user").value;
  const request_digest = byId("disk-add-digest").value;
  resetDiskAddition(); error(""); setBusy(true);
  try { showDiskAddition(service_user, await (await post("/api/disks/add-original",{service_user,request_digest,external_writers_stopped:byId("disk-add-writers").checked})).json()); }
  catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("disk-add-save").addEventListener("click", () => {
  if (!diskAdditionPreview) return;
  save(new Blob([JSON.stringify(diskAdditionPreview,null,2)],{type:"application/json"}),`zxs-addition-${diskAdditionPreview.input.request_digest}.json`);
});
byId("disk-add-commit-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !diskAdditionPreview) return;
  const retained = diskAdditionPreview;
  if (!confirm(`只添加 / 继续原操作 ${retained.plan.addition.request.operation_id}？中断可能已发布，不能清盘或换新编号。`)) return;
  const body = {...retained,confirmation:"add-and-initialize-empty-directories"};
  setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/add-commit",body)).json();
    byId("disk-add-result").textContent = JSON.stringify(result,null,2);
    byId("disk-add-result").hidden = false;
    diskAdditionPreview = null;
    byId("disk-add-commit-form").hidden = true;
  } catch (e) {
    error(`${e.message} 原请求摘要：${retained.input.request_digest}。先核对原操作，不能根据失败响应推断未写入。`);
    diskAdditionPreview = null;
    byId("disk-add-commit-form").hidden = true;
  } finally { setBusy(false); }
});

const foreignChainSelectorKey = "zxs-helper:foreign-chain-original";
function resetForeignChain() {
  foreignChainPreview = null;
  resetForeignIdentity();
  foreignChainDatabasesPreview = null;
  for (const id of ["foreign-chain-databases-preview","foreign-chain-databases-plan","foreign-chain-databases-save","foreign-chain-databases-form","foreign-chain-databases-result"]) byId(id).hidden = true;
  byId("foreign-chain-databases-confirm").checked = false;
  for (const id of ["foreign-chain-plan","foreign-chain-save","foreign-chain-prepare-form","foreign-chain-result"]) byId(id).hidden = true;
  byId("foreign-chain-confirm").checked = false;
}
function showForeignChain(service_user, preview) {
  foreignChainPreview = {input:{service_user,operation_id:preview.plan.binding.operation_id,request_digest:preview.request_digest,external_writers_stopped:true},plan:preview.plan};
  byId("foreign-chain-operation").value = preview.plan.binding.operation_id;
  byId("foreign-chain-digest").value = preview.request_digest;
  byId("foreign-chain-plan").textContent = JSON.stringify(preview,null,2);
  for (const id of ["foreign-chain-plan","foreign-chain-save"]) byId(id).hidden = false;
  byId("foreign-chain-prepare-form").hidden = preview.batch_material_prepared;
  byId("foreign-chain-databases-preview").hidden = !preview.batch_material_prepared;
}
for (const id of ["foreign-chain-user","foreign-chain-source","foreign-chain-slot","foreign-chain-upstream","foreign-chain-metadata","foreign-chain-lower","foreign-chain-writers","foreign-chain-operation","foreign-chain-digest"]) byId(id).addEventListener("input",resetForeignChain);
byId("foreign-chain-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping) return;
  if (byId("foreign-chain-digest").value && !confirm("已有原操作选择器。若曾开始准备，应核对原批次，不要新建。确定这里只预览另一个未开始的批次？")) return;
  resetForeignChain(); error("");
  let lower;
  try { lower = JSON.parse(byId("foreign-chain-lower").value); }
  catch { error("低级链必须是按层级列出全部真实目录的 JSON 数组。"); return; }
  const service_user = byId("foreign-chain-user").value;
  const operation_id = "op_" + Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2,"0")).join("");
  const input = {service_user,operation_id,source_root:byId("foreign-chain-source").value,source_slot_id:byId("foreign-chain-slot").value,source_upstream_slot_id:byId("foreign-chain-upstream").value,source_metadata_member:byId("foreign-chain-metadata").checked,lower,external_writers_stopped:byId("foreign-chain-writers").checked};
  setBusy(true);
  try { showForeignChain(service_user, await (await post("/api/disks/foreign-chain-preview",input)).json()); }
  catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("foreign-chain-original").addEventListener("click", async () => {
  if (busy || stopping) return;
  const input = {service_user:byId("foreign-chain-user").value,operation_id:byId("foreign-chain-operation").value,request_digest:byId("foreign-chain-digest").value,external_writers_stopped:byId("foreign-chain-writers").checked};
  resetForeignChain(); error(""); setBusy(true);
  try { showForeignChain(input.service_user, await (await post("/api/disks/foreign-chain-original",input)).json()); }
  catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("foreign-chain-save").addEventListener("click", () => {
  if (!foreignChainPreview) return;
  save(new Blob([JSON.stringify(foreignChainPreview,null,2)],{type:"application/json"}),`zxs-foreign-chain-${foreignChainPreview.input.operation_id}.json`);
});
byId("foreign-chain-prepare-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !foreignChainPreview) return;
  const retained = foreignChainPreview;
  if (!confirm(`只准备 / 继续原完整链 ${retained.input.operation_id} 的所有低级材料？不会正式加入任何成员。`)) return;
  const selector = JSON.stringify(retained.input);
  try {
    localStorage.setItem(foreignChainSelectorKey,selector);
    if (localStorage.getItem(foreignChainSelectorKey) !== selector) throw new Error("原选择器读回不符");
  } catch (e) { error(`尚未发送准备请求：无法保存浏览器原操作选择器（${e.message}）。请保存计划并在可用的浏览器存储环境继续。`); return; }
  setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/foreign-chain-prepare",{...retained,confirmation:"prepare-entire-foreign-lower-chain-material"})).json();
    byId("foreign-chain-result").textContent = JSON.stringify(result,null,2);
    byId("foreign-chain-result").hidden = false;
  } catch (e) {
    error(`${e.message} 原操作 ${retained.input.operation_id}，摘要 ${retained.input.request_digest}。保留全部目录，先核对原批次；失败响应不表示未写入。`);
  } finally {
    foreignChainPreview = null;
    byId("foreign-chain-prepare-form").hidden = true;
    setBusy(false);
  }
});
byId("foreign-chain-databases-preview").addEventListener("click", async () => {
  if (busy || stopping || !foreignChainPreview) return;
  const input = {...foreignChainPreview.input,external_writers_stopped:byId("foreign-chain-writers").checked};
  resetForeignIdentity();
  foreignChainDatabasesPreview = null;
  for (const id of ["foreign-chain-databases-plan","foreign-chain-databases-save","foreign-chain-databases-form","foreign-chain-databases-result"]) byId(id).hidden = true;
  byId("foreign-chain-databases-confirm").checked = false;
  setBusy(true); error("");
  try {
    const preview = await (await post("/api/disks/foreign-chain-databases-preview",input)).json();
    foreignChainDatabasesPreview = {input,expected:preview.expected,database_digest:preview.database_digest};
    byId("foreign-chain-databases-plan").textContent = JSON.stringify(preview,null,2);
    byId("foreign-chain-databases-plan").hidden = false;
    byId("foreign-chain-databases-save").hidden = false;
    byId("foreign-chain-databases-form").hidden = preview.original_database_scopes_prepared;
    byId("foreign-chain-identity-preview").hidden = !preview.original_database_scopes_prepared;
  } catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("foreign-chain-databases-save").addEventListener("click", () => {
  if (!foreignChainDatabasesPreview) return;
  save(new Blob([JSON.stringify(foreignChainDatabasesPreview,null,2)],{type:"application/json"}),`zxs-foreign-databases-${foreignChainDatabasesPreview.input.operation_id}.json`);
});
byId("foreign-chain-databases-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !foreignChainDatabasesPreview) return;
  const retained = foreignChainDatabasesPreview;
  if (!confirm(`只准备 ${retained.input.operation_id} 的全部原身份数据库？不会正式接纳或启动服务。`)) return;
  const selector = JSON.stringify(retained.input);
  try {
    localStorage.setItem(foreignChainSelectorKey,selector);
    if (localStorage.getItem(foreignChainSelectorKey) !== selector) throw new Error("选择器读回不符");
  } catch (e) { error(`尚未发送数据库准备：${e.message}`); return; }
  setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/foreign-chain-databases-prepare",{...retained,confirmation:"prepare-original-three-scope-databases-for-entire-chain"})).json();
    byId("foreign-chain-databases-result").textContent = JSON.stringify(result,null,2);
    byId("foreign-chain-databases-result").hidden = false;
    byId("foreign-chain-identity-preview").hidden = !result.original_database_scopes_prepared;
  } catch (e) {
    error(`${e.message} 原操作 ${retained.input.operation_id}，数据库摘要 ${retained.database_digest}。保留全部原目录并重新查询，不换操作或清理材料。`);
  } finally {
    foreignChainDatabasesPreview = null;
    byId("foreign-chain-databases-form").hidden = true;
    setBusy(false);
  }
});
function resetForeignIdentity() {
  resetForeignObjects();
  foreignChainIdentityPreview = null;
  for (const id of ["foreign-chain-identity-preview","foreign-chain-identity-plan","foreign-chain-identity-save"]) byId(id).hidden = true;
}
byId("foreign-chain-identity-preview").addEventListener("click", async () => {
  if (busy || stopping || !foreignChainPreview) return;
  resetForeignObjects();
  foreignChainIdentityPreview = null;
  for (const id of ["foreign-chain-identity-plan","foreign-chain-identity-save"]) byId(id).hidden = true;
  const input = {...foreignChainPreview.input,external_writers_stopped:byId("foreign-chain-writers").checked};
  setBusy(true); error("");
  try {
    const preview = await (await post("/api/disks/foreign-chain-identity-preview",input)).json();
    foreignChainIdentityPreview = preview;
    byId("foreign-chain-identity-plan").textContent = JSON.stringify(preview,null,2);
    byId("foreign-chain-identity-plan").hidden = false;
    byId("foreign-chain-identity-save").hidden = false;
    byId("foreign-chain-objects-preview").hidden = false;
  } catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("foreign-chain-identity-save").addEventListener("click", () => {
  if (busy || stopping || !foreignChainIdentityPreview || !foreignChainPreview) return;
  save(new Blob([JSON.stringify(foreignChainIdentityPreview,null,2)],{type:"application/json"}),`zxs-foreign-identity-${foreignChainPreview.input.operation_id}.json`);
});
function resetForeignObjects() {
  foreignObjectsEpoch += 1;
  foreignChainObjectsPreview = null;
  for (const id of ["foreign-chain-objects-preview","foreign-chain-objects-plan","foreign-chain-objects-save","foreign-chain-objects-form","foreign-chain-objects-result"]) byId(id).hidden = true;
  byId("foreign-chain-objects-confirm").checked = false;
}
byId("foreign-chain-objects-preview").addEventListener("click", async () => {
  if (busy || stopping || !foreignChainPreview || !foreignChainIdentityPreview) return;
  const input = {...foreignChainPreview.input,external_writers_stopped:byId("foreign-chain-writers").checked};
  resetForeignObjects();
  byId("foreign-chain-objects-preview").hidden = false;
  const epoch = foreignObjectsEpoch;
  setBusy(true); error("");
  try {
    const report = await (await post("/api/disks/foreign-chain-objects-preview",input)).json();
    if (epoch !== foreignObjectsEpoch) return;
    foreignChainObjectsPreview = {input,expected:report.expected,mapping_digest:report.mapping_digest};
    byId("foreign-chain-objects-plan").textContent = JSON.stringify(report,null,2);
    byId("foreign-chain-objects-plan").hidden = false;
    byId("foreign-chain-objects-save").hidden = false;
    byId("foreign-chain-objects-form").hidden = report.object_identity_material_prepared;
  } catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("foreign-chain-objects-save").addEventListener("click", () => {
  if (busy || stopping || !foreignChainObjectsPreview) return;
  save(new Blob([JSON.stringify(foreignChainObjectsPreview,null,2)],{type:"application/json"}),`zxs-foreign-objects-${foreignChainObjectsPreview.input.operation_id}.json`);
});
byId("foreign-chain-objects-form").addEventListener("submit", async event => {
  event.preventDefault();
  if (busy || stopping || !foreignChainObjectsPreview || !byId("foreign-chain-objects-confirm").checked) return;
  const retained = foreignChainObjectsPreview;
  if (!confirm(`只准备 ${retained.input.operation_id} 的完整对象映射？不导入权限、不生成联合 State、不正式纳入。`)) return;
  const selector = JSON.stringify(retained.input);
  try {
    localStorage.setItem(foreignChainSelectorKey,selector);
    if (localStorage.getItem(foreignChainSelectorKey) !== selector) throw new Error("选择器读回不符");
  } catch (e) { error(`尚未发送对象映射准备：${e.message}`); return; }
  setBusy(true); error("");
  try {
    const report = await (await post("/api/disks/foreign-chain-objects-prepare",{...retained,confirmation:"prepare-complete-chain-object-identities-without-admission"})).json();
    byId("foreign-chain-objects-result").textContent = JSON.stringify(report,null,2);
    byId("foreign-chain-objects-result").hidden = false;
  } catch (e) {
    error(`${e.message} 原操作 ${retained.input.operation_id}，对象摘要 ${retained.mapping_digest}。保留全部材料并查询原请求；连接失败不表示未写入，现池改变也不重新绑定。`);
  } finally {
    foreignChainObjectsPreview = null;
    byId("foreign-chain-objects-form").hidden = true;
    byId("foreign-chain-objects-save").hidden = true;
    byId("foreign-chain-objects-confirm").checked = false;
    setBusy(false);
  }
});
// The browser selects a request; native execution reads its root-owned plan
// and parent reservations.
try {
  const saved = localStorage.getItem(foreignChainSelectorKey);
  if (saved !== null) {
    if (saved.length > 2048) throw new Error("原操作选择器超过上限");
    const selector = JSON.parse(saved);
    if (typeof selector.service_user !== "string" || selector.service_user.length > 32 || !/^op_[0-9a-f]{32}$/.test(selector.operation_id) || !/^[0-9a-f]{64}$/.test(selector.request_digest)) throw new Error("原操作选择器无效");
    byId("foreign-chain-user").value = selector.service_user;
    byId("foreign-chain-operation").value = selector.operation_id;
    byId("foreign-chain-digest").value = selector.request_digest;
  }
} catch (e) { error(`未自动使用保存的外来链选择器：${e.message}。保留原文件，手动输入原操作和摘要。`); }

let terminalRemovalInspection = null;
function resetDiskRemoval() {
  diskRemovalPreview = null;
  terminalRemovalInspection = null;
  for (const id of ["disk-remove-plan","disk-remove-save","disk-remove-commit-form","disk-remove-result"]) byId(id).hidden = true;
  byId("disk-remove-confirm").checked = false;
  byId("disk-remove-terminal").checked = false;
  byId("disk-remove-terminal").required = false;
  byId("disk-remove-terminal-label").hidden = true;
}
function showDiskRemoval(service_user,preview) {
  byId("disk-remove-save").textContent = "保存原注销计划";
  diskRemovalPreview = {input:{service_user,request_digest:preview.request_digest,external_writers_stopped:true},plan:preview.plan};
  byId("disk-remove-digest").value = preview.request_digest;
  byId("disk-remove-plan").textContent = JSON.stringify(preview,null,2);
  const terminal = preview.plan.exit.authority === "terminal_management";
  byId("disk-remove-terminal-label").hidden = !terminal;
  byId("disk-remove-terminal").required = terminal;
  for (const id of ["disk-remove-plan","disk-remove-save","disk-remove-commit-form"]) byId(id).hidden = false;
}
for (const id of ["disk-remove-user","disk-remove-writers","disk-remove-digest","disk-remove-target"]) byId(id).addEventListener("input",resetDiskRemoval);
byId("disk-remove-user").addEventListener("input", () => {
  byId("disk-remove-target").replaceChildren(new Option("先读取准确服务账户的登记", ""));
  delete byId("disk-remove-target").dataset.authority;
  byId("disk-remove-inventory").hidden = true;
});
byId("disk-remove-load").addEventListener("click", async () => {
  if (busy || stopping) return;
  resetDiskRemoval(); error(""); setBusy(true);
  const service_user = byId("disk-remove-user").value;
  byId("disk-remove-target").replaceChildren(new Option("选择准确当前登记", ""));
  byId("disk-remove-inventory").hidden = true;
  try {
    const inventory = await (await post("/api/disks/registrations",{service_user,external_writers_stopped:byId("disk-remove-writers").checked})).json();
    byId("disk-remove-target").dataset.authority = inventory.authority;
    for (const item of inventory.registrations) {
      const r = item.registration;
      const label = `${r.slot_id} · ${r.disk_id} · ${r.mode}（登记状态，不代表健康） · ${item.root_path}`;
      const option = new Option(label,JSON.stringify({slot_id:r.slot_id,expected_disk_id:r.disk_id}));

      byId("disk-remove-target").append(option);
    }
    byId("disk-remove-inventory").textContent = JSON.stringify(inventory,null,2);
    byId("disk-remove-inventory").hidden = false;
    if (inventory.authority === "terminal_management") {
      error("当前为终态管理权威，没有活动存储。可以选择剩余登记检查其准确权威边界，下方可另行预览准确的逐笔终态后继；原摘要查询/续作仍单独保留。");
    }
  } catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("disk-remove-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping) return;
  if (!byId("disk-remove-target").value) { error("先读取并选择准确的当前登记。"); return; }
  const target = JSON.parse(byId("disk-remove-target").value);
  const service_user = byId("disk-remove-user").value;
  const terminal = byId("disk-remove-target").dataset.authority === "terminal_management";
  resetDiskRemoval(); error(""); setBusy(true);
  try {
    if (terminal) {
      const inspection = await (await post("/api/disks/terminal-removal-inspect",{service_user,...target,external_writers_stopped:byId("disk-remove-writers").checked})).json();
      terminalRemovalInspection = inspection;
      byId("disk-remove-plan").textContent = JSON.stringify(inspection,null,2);
      byId("disk-remove-plan").hidden = false;
      byId("disk-remove-save").textContent = "保存不可执行的终态检查报告";
      byId("disk-remove-save").hidden = false;
      error("检查完成，但没有创建注销操作。此检查不能提交、擦盘或据此拔盘；下方逐笔后继须重新预览和确认，该检查摘要不是操作/准入令牌。");
    } else {
      const operation_id = "op_" + Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2,"0")).join("");
      showDiskRemoval(service_user, await (await post("/api/disks/deregister-preview",{service_user,operation_id,...target,external_writers_stopped:byId("disk-remove-writers").checked})).json());
    }
  }
  catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("disk-remove-original").addEventListener("click", async () => {
  if (busy || stopping) return;
  const service_user = byId("disk-remove-user").value;
  const request_digest = byId("disk-remove-digest").value;
  resetDiskRemoval(); error(""); setBusy(true);
  try { showDiskRemoval(service_user, await (await post("/api/disks/deregister-original",{service_user,request_digest,external_writers_stopped:byId("disk-remove-writers").checked})).json()); }
  catch (e) { error(e.message); } finally { setBusy(false); }
});
byId("disk-remove-save").addEventListener("click", () => {
  if (diskRemovalPreview) save(new Blob([JSON.stringify(diskRemovalPreview,null,2)],{type:"application/json"}),`zxs-deregister-${diskRemovalPreview.input.request_digest}.json`);
  else if (terminalRemovalInspection) save(new Blob([JSON.stringify(terminalRemovalInspection,null,2)],{type:"application/json"}),`zxs-terminal-inspection-${terminalRemovalInspection.inspection_digest}.json`);
});
byId("disk-remove-commit-form").addEventListener("submit", async event => {
  event.preventDefault(); if (busy || stopping || !diskRemovalPreview) return;
  const retained = diskRemovalPreview;
  const terminal = retained.plan.exit.authority === "terminal_management";
  const consequence = terminal ? "这是最后一个元数据成员：活动存储会停止，进入终态管理，不会自动恢复。" : "当前归属和内容访问可能丢失。";
  if (!confirm(`注销 ${retained.plan.exit.target.slot_id} / ${retained.plan.exit.target.disk_id}？保留盘字节。${consequence}只继续原操作 ${retained.plan.exit.request.operation_id}。`)) return;
  setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/deregister-commit",{...retained,confirmation:terminal ? "deregister-final-voter-stop-storage" : "deregister-member-preserve-bytes",acknowledge_current_view_loss:byId("disk-remove-confirm").checked,acknowledge_terminal_management:byId("disk-remove-terminal").checked})).json();
    byId("disk-remove-result").textContent = JSON.stringify(result,null,2);
    byId("disk-remove-result").hidden = false;
    byId("disk-remove-target").replaceChildren(new Option("注销后请重新读取当前登记", ""));
  } catch (e) { error(`${e.message} 原摘要：${retained.input.request_digest}。结果不明时不要拔盘或换编号重做，先核对原操作。`); }
  finally { diskRemovalPreview = null; byId("disk-remove-commit-form").hidden = true; setBusy(false); }
});

// This is UI state only. Native SYSTEM owns decision, Stop and final receipts.
let terminalRetirementPreview = null;
function resetTerminalRetirement() {
  terminalRetirementPreview = null;
  for (const id of ["terminal-retire-plan","terminal-retire-save","terminal-retire-commit","terminal-retire-result"]) byId(id).hidden = true;
  byId("terminal-retire-loss").checked = false;
  byId("terminal-retire-restart").checked = false;
}
function showTerminalRetirement(service_user, preview) {
  terminalRetirementPreview = {input:{service_user,request_digest:preview.request_digest,external_writers_stopped:true},plan:preview.plan};
  byId("terminal-retire-digest").value = preview.request_digest;
  byId("terminal-retire-plan").textContent = JSON.stringify(preview,null,2);
  for (const id of ["terminal-retire-plan","terminal-retire-save"]) byId(id).hidden = false;
  byId("terminal-retire-commit").hidden = preview.original_result !== null;
}
for (const id of ["disk-remove-user","disk-remove-target","disk-remove-writers","terminal-retire-digest"]) byId(id).addEventListener("input",resetTerminalRetirement);
byId("disk-remove-load").addEventListener("click",resetTerminalRetirement);
byId("terminal-retire-preview").addEventListener("click",async () => {
  if (busy || stopping) return;
  if (byId("disk-remove-target").dataset.authority !== "terminal_management" || !byId("disk-remove-target").value) {
    error("先在上方读取并选择准确终态登记，不从配置或旧报告推断登记。"); return;
  }
  const target = JSON.parse(byId("disk-remove-target").value);
  const service_user = byId("disk-remove-user").value;
  const operation_id = "op_" + Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2,"0")).join("");
  resetTerminalRetirement(); error(""); setBusy(true);
  try { showTerminalRetirement(service_user,await (await post("/api/disks/terminal-retirement-preview",{service_user,operation_id,...target,external_writers_stopped:byId("disk-remove-writers").checked})).json()); }
  catch(e) { error(e.message); } finally { setBusy(false); }
});
byId("terminal-retire-original").addEventListener("click",async () => {
  if (busy || stopping) return;
  const service_user = byId("disk-remove-user").value;
  const request_digest = byId("terminal-retire-digest").value;
  resetTerminalRetirement(); error(""); setBusy(true);
  try { showTerminalRetirement(service_user,await (await post("/api/disks/terminal-retirement-original",{service_user,request_digest,external_writers_stopped:byId("disk-remove-writers").checked})).json()); }
  catch(e) { error(e.message); } finally { setBusy(false); }
});
byId("terminal-retire-save").addEventListener("click",() => {
  if (terminalRetirementPreview) save(new Blob([JSON.stringify(terminalRetirementPreview,null,2)],{type:"application/json"}),`zxs-terminal-retirement-${terminalRetirementPreview.input.request_digest}.json`);
});
byId("terminal-retire-commit").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping || !terminalRetirementPreview) return;
  const retained = terminalRetirementPreview;
  const native = retained.plan.retirement;
  if (!confirm(`移除终态登记 ${native.target.slot_id} / ${native.target.disk_id}？只继续 ${native.request.operation_id}。保留物理字节；本阶段完成后不支持元数据重启。`)) return;
  setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/terminal-retirement-commit",{...retained,confirmation:"retire-terminal-member-without-metadata-restart",acknowledge_current_view_loss:byId("terminal-retire-loss").checked,acknowledge_no_metadata_resumption:byId("terminal-retire-restart").checked})).json();
    byId("terminal-retire-result").textContent = JSON.stringify(result,null,2);
    byId("terminal-retire-result").hidden = false;
    byId("disk-remove-target").replaceChildren(new Option("请重新读取实际登记", ""));
  } catch(e) { error(`${e.message} 原终态摘要：${retained.input.request_digest}。可能已部分或全部完成，不删证据或换编号；先核对同一原操作。`); }
  finally { terminalRetirementPreview = null; byId("terminal-retire-commit").hidden = true; setBusy(false); }
});


function resetDiskReadmission() {
  diskReadmissionPreview = null;
  for (const id of ["disk-readmit-plan","disk-readmit-save","disk-readmit-commit","disk-readmit-result"]) byId(id).hidden = true;
  byId("disk-readmit-epoch").checked = false;
}
function showDiskReadmission(service_user, preview) {
  diskReadmissionPreview = {input:{service_user,request_digest:preview.request_digest,external_writers_stopped:true},plan:preview.plan};
  byId("disk-readmit-digest").value = preview.request_digest;
  byId("disk-readmit-plan").textContent = JSON.stringify(preview,null,2);
  for (const id of ["disk-readmit-plan","disk-readmit-save"]) byId(id).hidden = false;
  byId("disk-readmit-commit").hidden = preview.original_status !== null && preview.original_status.state === "completed";
}
for (const id of ["disk-remove-user","disk-remove-target","disk-remove-writers","disk-readmit-digest"]) byId(id).addEventListener("input",resetDiskReadmission);
byId("disk-remove-load").addEventListener("click",resetDiskReadmission);
for (const id of ["disk-remove-commit-form","terminal-retire-commit","disk-add-commit-form"]) byId(id).addEventListener("submit",resetDiskReadmission);
byId("disk-readmit-preview").addEventListener("click",async () => {
  if (busy || stopping) return;
  if (byId("disk-remove-target").dataset.authority !== "terminal_management" || !byId("disk-remove-target").value) {
    error("先在上方读取并选择仍登记的原终态元数据盘；不能根据旧配置、外来材料或已注销身份推断可重新接纳。"); return;
  }
  const target = JSON.parse(byId("disk-remove-target").value);
  const service_user = byId("disk-remove-user").value;
  const operation_id = "op_" + Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2,"0")).join("");
  resetDiskReadmission(); resetDiskRemoval(); resetTerminalRetirement(); error(""); setBusy(true);
  try { showDiskReadmission(service_user,await (await post("/api/disks/readmission-preview",{service_user,operation_id,...target,external_writers_stopped:byId("disk-remove-writers").checked})).json()); }
  catch(e) { error(e.message); } finally { setBusy(false); }
});
byId("disk-readmit-original").addEventListener("click",async () => {
  if (busy || stopping) return;
  const service_user = byId("disk-remove-user").value;
  const request_digest = byId("disk-readmit-digest").value;
  resetDiskReadmission(); resetDiskRemoval(); resetTerminalRetirement(); error(""); setBusy(true);
  try { showDiskReadmission(service_user,await (await post("/api/disks/readmission-original",{service_user,request_digest,external_writers_stopped:byId("disk-remove-writers").checked})).json()); }
  catch(e) { error(e.message); } finally { setBusy(false); }
});
byId("disk-readmit-save").addEventListener("click",() => {
  if (diskReadmissionPreview) save(new Blob([JSON.stringify(diskReadmissionPreview,null,2)],{type:"application/json"}),`zxs-terminal-readmission-${diskReadmissionPreview.input.request_digest}.json`);
});
byId("disk-readmit-commit").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping || !diskReadmissionPreview) return;
  const retained = diskReadmissionPreview;
  const native = retained.plan.readmission;
  if (!confirm(`重新接纳原目录 ${native.target.slot_id} / ${native.target.disk_id} 并签发新 I/O epoch？只继续 ${native.request.operation_id}。不会启动服务，也不代表全池健康。`)) return;
  setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/readmission-commit",{...retained,confirmation:"readmit-retained-terminal-metadata",acknowledge_original_directory_and_new_epoch:byId("disk-readmit-epoch").checked})).json();
    byId("disk-readmit-result").textContent = JSON.stringify(result,null,2);
    byId("disk-readmit-result").hidden = false;
  } catch(e) { error(`${e.message} 原 readmission 摘要：${retained.input.request_digest}。可能已部分或全部发布，请先核对同一原操作，不换盘或编号重做。`); }
  finally {
    diskReadmissionPreview = null;
    byId("disk-readmit-commit").hidden = true;
    byId("disk-readmit-save").hidden = true;
    resetDiskRemoval(); resetTerminalRetirement(); resetDiskAddition();
    byId("disk-remove-target").replaceChildren(new Option("请重新读取实际登记", ""));
    delete byId("disk-remove-target").dataset.authority;
    byId("disk-remove-inventory").hidden = true;
    setBusy(false);
  }
});

byId("terminal-chain-export").addEventListener("click",async () => {
  if (busy || stopping) return;
  const service_user = byId("disk-remove-user").value;
  byId("terminal-chain-report").hidden = true;
  setBusy(true); error("");
  try {
    const archive = await (await post("/api/disks/terminal-chain-export",{service_user,external_writers_stopped:byId("disk-remove-writers").checked})).json();
    const {payload, ...report} = archive;
    byId("terminal-chain-report").textContent = JSON.stringify(report,null,2);
    byId("terminal-chain-report").hidden = false;
    save(new Blob([payload],{type:"application/json"}),`zxs-terminal-chain-${archive.checkpoint.digest}.json`);
  } catch(e) { error(`${e.message} 未导出完整检查点；不把部分内容或旧报告当成接纳成功。`); }
  finally { setBusy(false); }
});
for (const id of ["disk-remove-user","disk-remove-target","disk-remove-writers"]) byId(id).addEventListener("input", () => { byId("terminal-chain-report").hidden = true; });


byId("terminal-chain-readmit").addEventListener("click",async () => {
  if (busy || stopping) return;
  if (byId("disk-remove-target").dataset.authority !== "terminal_management" || !byId("disk-remove-target").value) {
    error("先读取并选择准确终态登记；本入口只生成完整来源提案，不执行重新接纳。"); return;
  }
  const target = JSON.parse(byId("disk-remove-target").value);
  const operation_id = "op_" + Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2,"0")).join("");
  byId("terminal-chain-readmit-report").hidden = true;
  resetChainTarget();
  setBusy(true); error("");
  try {
    const archive = await (await post("/api/disks/terminal-chain-readmission-proposal", {
      service_user:byId("disk-remove-user").value, operation_id, ...target,
      external_writers_stopped:byId("disk-remove-writers").checked
    })).json();
    const {payload,...report} = archive;
    byId("terminal-chain-readmit-report").textContent = JSON.stringify(report,null,2);
    byId("terminal-chain-readmit-report").hidden = false;
    save(new Blob([payload],{type:"application/json"}),`zxs-terminal-chain-readmission-${archive.preparation.proposal_digest}.json`);
    chainTargetPlan = archive.plan;
    byId("chain-target-form").hidden = false;
  } catch(e) { error(`${e.message} 未生成完整恢复提案，没有新 readmission 决定。此入口打开来源时仍可重复已有恢复。`); }
  finally { setBusy(false); }
});
for (const id of ["disk-remove-user","disk-remove-target","disk-remove-writers"]) byId(id).addEventListener("input", () => { byId("terminal-chain-readmit-report").hidden = true; });
byId("disk-remove-load").addEventListener("click", () => { byId("terminal-chain-readmit-report").hidden = true; });
for (const id of ["terminal-retire-commit","disk-readmit-commit","disk-remove-commit-form"]) byId(id).addEventListener("submit", () => { byId("terminal-chain-readmit-report").hidden = true; });


function resetChainTarget() {
  chainTargetPlan = null;
  byId("chain-generation-select-form").hidden = true;
  byId("chain-generation-select-ack").checked = false;
  byId("chain-generation-read").hidden = true;
  byId("chain-generation-log-form").hidden = true;
  byId("chain-generation-log-ack").checked = false;
  byId("chain-io-decision-form").hidden = true;
  byId("chain-io-decision-ack").checked = false;
  byId("chain-current-read").hidden = true;
  byId("chain-epoch-form").hidden = true;
  byId("chain-epoch-ack").checked = false;
  byId("chain-target-form").hidden = true;
  byId("chain-target-ack").checked = false;
  byId("chain-target-result").hidden = true;
  byId("chain-first-form").hidden = true;
  byId("chain-first-ack").checked = false;
  byId("chain-publish-form").hidden = true;
  byId("chain-publish-ack").checked = false;
}
byId("chain-target-status").addEventListener("click",async () => {
  if (busy || stopping) return;
  resetChainTarget(); setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/terminal-chain-target-status", {
      service_user:byId("disk-remove-user").value, external_writers_stopped:byId("disk-remove-writers").checked
    })).json();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
    chainTargetPlan = result.plan;
    const publishedPhase = ["first-publication-preserved", "activation-plan-selected","io-decision-pending","io-decision-preserved","generation-log-pending","generation-log-preserved","generation-selection-pending","generation-selected"].includes(result.native?.phase);
    byId("chain-current-read").hidden = !chainTargetPlan || !publishedPhase;
    byId("chain-epoch-form").hidden = !chainTargetPlan || !publishedPhase;
    byId("chain-io-decision-form").hidden = !chainTargetPlan || !["activation-plan-selected","io-decision-pending","io-decision-preserved","generation-log-pending","generation-log-preserved","generation-selection-pending","generation-selected"].includes(result.native?.phase);
    byId("chain-generation-log-form").hidden = !chainTargetPlan || !["io-decision-preserved","generation-log-pending","generation-log-preserved","generation-selection-pending","generation-selected"].includes(result.native?.phase);
    byId("chain-generation-select-form").hidden = !chainTargetPlan || !["generation-log-preserved","generation-selection-pending","generation-selected"].includes(result.native?.phase);
    byId("chain-generation-read").hidden = !chainTargetPlan || result.native?.phase !== "generation-selected";
    byId("chain-target-form").hidden = !chainTargetPlan;
    const publicationStarted = ["first-publication-pending","first-publication-preserved","activation-plan-selected","io-decision-pending","io-decision-preserved","generation-log-pending","generation-log-preserved","generation-selection-pending","generation-selected"].includes(result.native?.phase);
    byId("chain-first-form").hidden = !chainTargetPlan || !result.native?.target_decision_hash || publicationStarted;
    byId("chain-publish-form").hidden = !chainTargetPlan || !["first-candidate-prepared","first-publication-pending","first-publication-preserved","activation-plan-selected","io-decision-pending","io-decision-preserved","generation-log-pending","generation-log-preserved","generation-selection-pending","generation-selected"].includes(result.native?.phase);
  } catch(e) { error(`${e.message} 状态不明不代表未写入；保留原生证据，不换编号或盘。`); }
  finally { setBusy(false); }
});
byId("chain-target-form").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping || !chainTargetPlan) return;
  const plan = chainTargetPlan;
  if (!confirm(`只准备原目标 ${plan.preparation.target.slot_id} / ${plan.preparation.target.disk_id} 的决定？将保留 ${plan.preparation.operation_id} 并阻止新终态移除；尚不能激活服务，没有取消或换盘接口。`)) return;
  setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/terminal-chain-target-synchronize", {
      service_user:byId("disk-remove-user").value, external_writers_stopped:byId("disk-remove-writers").checked,
      plan, confirmation:"prepare-chain-target-without-activation", acknowledge_reserved_request_without_activation:byId("chain-target-ack").checked
    })).json();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
  } catch(e) { error(`${e.message} 可能已保留来源或写入目标决定；原 operation ${plan.preparation.operation_id}，提案 ${plan.preparation.proposal_digest}。先查询原状态，不清理证据或换盘重做。`); }
  finally {
    chainTargetPlan = null; byId("chain-target-form").hidden = true;
    byId("chain-first-form").hidden = true; byId("chain-first-ack").checked = false;
    byId("chain-publish-form").hidden = true; byId("chain-publish-ack").checked = false;
    byId("chain-target-ack").checked = false;
    byId("terminal-chain-readmit-report").hidden = true;
    byId("terminal-chain-report").hidden = true;
    resetDiskReadmission(); resetTerminalRetirement(); resetDiskRemoval();
    setBusy(false);
  }
});
for (const id of ["disk-remove-user","disk-remove-target","disk-remove-writers"]) byId(id).addEventListener("input",resetChainTarget);
byId("disk-remove-load").addEventListener("click",resetChainTarget);
for (const id of ["terminal-retire-commit","disk-readmit-commit","disk-remove-commit-form"]) byId(id).addEventListener("submit",resetChainTarget);

byId("chain-first-form").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping || !chainTargetPlan) return;
  const plan = chainTargetPlan;
  if (!confirm(`仅为原操作 ${plan.preparation.operation_id} 准备首笔原生输入？SYSTEM 和原目标 ${plan.preparation.target.slot_id} 各存完整隔离材料，不真正发布、接纳 writer 或启动服务。`)) return;
  setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/terminal-chain-first-prepare", {
      service_user:byId("disk-remove-user").value,
      external_writers_stopped:byId("disk-remove-writers").checked,
      plan, confirmation:"prepare-chain-first-inputs-without-publication",
      acknowledge_unpublished_inputs:byId("chain-first-ack").checked
    })).json();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
  } catch(e) { error(`${e.message} 首笔选择或部分原生材料可能已落盘；继续原 operation ${plan.preparation.operation_id} / ${plan.preparation.proposal_digest}，不清理或换编号。`); }
  finally {
    chainTargetPlan = null;
    byId("chain-first-form").hidden = true; byId("chain-first-ack").checked = false;
    byId("chain-publish-form").hidden = true; byId("chain-publish-ack").checked = false;
    byId("chain-target-form").hidden = true; byId("chain-target-ack").checked = false;
    byId("terminal-chain-readmit-report").hidden = true;
    byId("terminal-chain-report").hidden = true;
    resetDiskReadmission(); resetTerminalRetirement(); resetDiskRemoval();
    setBusy(false);
  }
});

byId("chain-publish-form").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping || !chainTargetPlan) return;
  const plan = chainTargetPlan;
  if (!confirm(`真实提交原操作 ${plan.preparation.operation_id} 的首笔 ACCEPT/FINALIZE/Publication？不是仅准备，无取消或换盘；仍不会恢复活动 writer 或服务。目标 ${plan.preparation.target.slot_id} / ${plan.preparation.target.disk_id}。`)) return;
  setBusy(true); error("");
  try {
    const result = await (await post("/api/disks/terminal-chain-first-publish", {
      service_user:byId("disk-remove-user").value,
      external_writers_stopped:byId("disk-remove-writers").checked,
      plan, confirmation:"publish-chain-first-slot-without-activation",
      acknowledge_publication_without_activation:byId("chain-publish-ack").checked
    })).json();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
  } catch(e) { error(`${e.message} 原生首笔可能已经真实发布；原 operation ${plan.preparation.operation_id} / ${plan.preparation.proposal_digest}，保留全部证据，只查询/恢复原发布，不换盘、清理或换编号。`); }
  finally {
    chainTargetPlan = null;
    for (const id of ["chain-publish-form","chain-first-form","chain-target-form"]) byId(id).hidden = true;
    for (const id of ["chain-publish-ack","chain-first-ack","chain-target-ack"]) byId(id).checked = false;
    byId("terminal-chain-readmit-report").hidden = true; byId("terminal-chain-report").hidden = true;
    resetDiskReadmission(); resetTerminalRetirement(); resetDiskRemoval();
    setBusy(false);
  }
});

byId("chain-current-read").addEventListener("click", async () => {
  if (busy || stopping || !chainTargetPlan) return;
  const plan = chainTargetPlan;
  setBusy(true); error("");
  byId("chain-target-result").hidden = true;
  try {
    const result = await (await post("/api/disks/terminal-chain-current", {
      service_user:byId("disk-remove-user").value,
      external_writers_stopped:byId("disk-remove-writers").checked,
      plan
    })).json();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
  } catch(e) {
    error(`${e.message} 不能确认原目标冻结 Current；这不是未发布或可清理的证明。保留原请求和全部证据，不启动服务。`);
  } finally { setBusy(false); }
});

byId("chain-epoch-form").addEventListener("submit", async event => {
  event.preventDefault();
  if (busy || stopping || !chainTargetPlan) return;
  const plan = chainTargetPlan;
  if (!window.confirm("仅保存这个原请求的新 epoch/Stop 后继计划，不更新活动 locator、不消费 Stop、不启用 writer 或启动服务。继续？")) return;
  setBusy(true); error("");
  byId("chain-target-result").hidden = true;
  try {
    const result = await (await post("/api/disks/terminal-chain-activation-plan", {
      service_user:byId("disk-remove-user").value,
      external_writers_stopped:byId("disk-remove-writers").checked,
      plan,
      confirmation:"prepare-chain-activation-plan-without-consuming-stop",
      acknowledge_plan_without_stop_consumption:byId("chain-epoch-ack").checked
    })).json();
    resetChainTarget();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
  } catch(e) {
    resetChainTarget();
    error(`${e.message} 计划可能已经保留。只查询并恢复原请求 ${plan.preparation.operation_id}，保留 Publication/Stop 和完整原证据，不能据此启动服务。`);
  } finally { setBusy(false); }
});

byId("chain-io-decision-form").addEventListener("submit", async event => {
  event.preventDefault();
  if (busy || stopping || !chainTargetPlan) return;
  const plan = chainTargetPlan;
  if (!window.confirm(`在原盘为请求 ${plan.preparation.operation_id} 作出不可改写的新 epoch 决定？不能撤销、换盘或另选 epoch；仍不消费 Stop、不启用 writer 或启动服务。`)) return;
  setBusy(true); error("");
  byId("chain-target-result").hidden = true;
  try {
    const result = await (await post("/api/disks/terminal-chain-io-decide", {
      service_user:byId("disk-remove-user").value,
      external_writers_stopped:byId("disk-remove-writers").checked,
      plan,
      confirmation:"decide-chain-io-epoch-without-activation",
      acknowledge_irreversible_decision_without_activation:byId("chain-io-decision-ack").checked
    })).json();
    resetChainTarget();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
  } catch(e) {
    resetChainTarget();
    error(`${e.message} 原盘可能已作出不可撤销的 epoch 决定。保留原请求 ${plan.preparation.operation_id}、实际 Publication、Stop 和全部证据；只能查询/续作原决定，不能据此启动服务或换盘。`);
  } finally { setBusy(false); }
});

byId("chain-generation-log-form").addEventListener("submit", async event => {
  event.preventDefault();
  if (busy || stopping || !chainTargetPlan) return;
  const plan = chainTargetPlan;
  if (!window.confirm(`为原请求 ${plan.preparation.operation_id} 安装实际 generation 日志及完整原生对象？需要额外空间，不能换盘或清理证据；仍不消费 Stop、不启用活动 writer 或启动服务。`)) return;
  setBusy(true); error("");
  byId("chain-target-result").hidden = true;
  try {
    const result = await (await post("/api/disks/terminal-chain-generation-log", {
      service_user:byId("disk-remove-user").value,
      external_writers_stopped:byId("disk-remove-writers").checked,
      plan,
      confirmation:"prepare-chain-generation-log-without-active-writer",
      acknowledge_generation_log_without_active_writer:byId("chain-generation-log-ack").checked
    })).json();
    resetChainTarget();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
  } catch(e) {
    resetChainTarget();
    error(`${e.message} 原目标可能已保存部分或全部 generation 日志/对象。只查询并续作原请求 ${plan.preparation.operation_id}，保留全部证据，不启动服务、不清理或另换盘。`);
  } finally { setBusy(false); }
});

byId("chain-generation-select-form").addEventListener("submit", async event => {
  event.preventDefault();
  if (busy || stopping || !chainTargetPlan) return;
  const plan = chainTargetPlan;
  if (!window.confirm(`为原请求 ${plan.preparation.operation_id} 选择已经完整安装的准确 generation？只保留冷恢复选择和冻结读消费者，不消费 Stop、不启 writer 或服务，失败仅原请求续作。`)) return;
  const acknowledged = byId("chain-generation-select-ack").checked;
  setBusy(true); error("");
  byId("chain-target-result").hidden = true;
  try {
    const result = await (await post("/api/disks/terminal-chain-generation-select", {
      service_user:byId("disk-remove-user").value,
      external_writers_stopped:byId("disk-remove-writers").checked,
      plan,
      confirmation:"select-chain-generation-without-active-writer",
      acknowledge_selector_without_active_writer:acknowledged
    })).json();
    resetChainTarget();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
  } catch(e) {
    resetChainTarget();
    error(`${e.message} 原 selector 可能已部分或全部保存。只查询/续作 ${plan.preparation.operation_id}，不换盘、清理证据或启动服务。`);
  } finally { setBusy(false); }
});
byId("chain-generation-read").addEventListener("click", async () => {
  if (busy || stopping || !chainTargetPlan) return;
  const plan = chainTargetPlan;
  setBusy(true); error("");
  byId("chain-target-result").hidden = true;
  try {
    const result = await (await post("/api/disks/terminal-chain-generation-read", {
      service_user:byId("disk-remove-user").value,
      external_writers_stopped:byId("disk-remove-writers").checked,
      plan
    })).json();
    resetChainTarget();
    byId("chain-target-result").textContent = JSON.stringify(result,null,2);
    byId("chain-target-result").hidden = false;
  } catch(e) {
    resetChainTarget();
    error(`${e.message} 未修复或补造 selector/日志/对象。保留 ${plan.preparation.operation_id} 的原证据，不能据此启用 writer 或服务。`);
  } finally { setBusy(false); }
});
