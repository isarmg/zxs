"use strict";
const byId = id => document.getElementById(id);
let csrf = null;
let busy = false;
let stopping = false;
let installationPreview = null;
let deploymentPreview = null;
let hostPreview = null;
let layoutPreview = null;
let configurationPreview = null;
let administratorResetPreview = null;
let firstAdministratorPreview = null;
let firstStoragePreview = null;
let diskAdditionPreview = null;
let diskRemovalPreview = null;
let diskReadmissionPreview = null;
let sambaPasswordPreview = null;
let sambaPasswordOriginal = null;
const maxSelectionBytes = 60 * 1024;
function error(message) { byId("error").textContent = message; byId("error").hidden = !message; }
function setBusy(value) { busy = value; byId("terminal-chain-export").disabled = value || stopping || !csrf; for (const input of byId("disk-readmit-commit").elements) input.disabled = value || stopping || !csrf; for (const id of ["disk-readmit-preview","disk-readmit-original","disk-readmit-digest","disk-readmit-save"]) byId(id).disabled = value || stopping || !csrf; for (const input of byId("terminal-retire-commit").elements) input.disabled = value || stopping || !csrf; for (const id of ["terminal-retire-preview","terminal-retire-original","terminal-retire-digest","terminal-retire-save"]) byId(id).disabled = value || stopping || !csrf; for (const form of ["disk-remove-form","disk-remove-commit-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const form of ["disk-add-form","disk-add-commit-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const form of ["samba-reset-form","samba-password-form","samba-reset-status-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const form of ["first-storage-form", "first-storage-commit-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const input of byId("first-administrator-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("first-administrator-password-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("administrator-reset-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("administrator-password-form").elements) input.disabled = value || stopping || !csrf; byId("check").disabled = value || stopping || !csrf; byId("selection").disabled = value || stopping; byId("selection-file").disabled = value || stopping; byId("install-preview").disabled = value || stopping || !csrf; byId("install-commit").disabled = value || stopping || !csrf; for (const input of byId("deployment-form").elements) input.disabled = value || stopping || !csrf; byId("deployment-prepare").disabled = value || stopping || !csrf; byId("configuration-install").disabled = value || stopping || !csrf; byId("external-administration-stopped").disabled = value || stopping; byId("host-preview").disabled = value || stopping || !csrf; byId("host-prepare").disabled = value || stopping || !csrf; for (const input of byId("layout-form").elements) input.disabled = value || stopping || !csrf; byId("layout-prepare").disabled = value || stopping || !csrf; }
function resetResults() { byId("results").hidden = true; byId("copies").replaceChildren(); byId("report").textContent = ""; error(""); }
async function status() {
  const response = await fetch("/api/status", {cache:"no-store", credentials:"omit"});
  if (!response.ok) throw new Error(`无法连接辅助工具（HTTP ${response.status}）。请使用主机的 IP 和实际端口直接打开，不使用 DNS 名称或代理。`);
  const state = await response.json(); csrf = state.csrf; stopping = state.stopping;
  byId("connection").textContent = stopping ? "辅助工具正在等待已开始的读取结束并退出。" : `${state.name} ${state.version} · 独立维护模式 · 无登录 · ${state.busy ? "已有维护任务" : "没有进行中的维护任务"}`;
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

function resetDeployment() {
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

function resetConfiguration() {
  configurationPreview = null;
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
  if (!window.confirm("确认仅安装预览中的 /etc/systemd/system/zxs.service 和可选 /etc/samba/zxs.conf？不覆盖不同文件，不更改默认 smb.conf 或密码，不加载/启动/启用服务。当前未提供完整初始化和自动激活。")) return;
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
byId("deployment-form").addEventListener("input",invalidateSambaPreview);
byId("samba-writers-stopped").addEventListener("change",invalidateSambaPreview);
byId("samba-reset-form").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping) return;
  invalidateSambaPreview(); error("");
  let input;
  try {
    const nonce = crypto.getRandomValues(new Uint8Array(16));
    const operation_id = Array.from(nonce,byte => byte.toString(16).padStart(2,"0")).join("");
    input = {deployment:sambaDeploymentInput(),operation_id,external_writers_stopped:byId("samba-writers-stopped").checked};
  } catch (failure) { error(failure.message); return; }
  setBusy(true);
  try {
    const preview = await (await post("/api/credentials/samba-preview",input)).json();
    sambaPasswordPreview = {input,request_digest:preview.request_digest};
    sambaPasswordOriginal = sambaPasswordPreview;
    byId("samba-reset-digest").value = preview.request_digest;
    byId("samba-reset-plan").textContent = JSON.stringify(preview,null,2);
    for (const id of ["samba-reset-plan","samba-reset-save","samba-password-form"]) byId(id).hidden = false;
    byId("samba-reset-receipt").hidden = true;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
byId("samba-reset-save").addEventListener("click",() => {
  if (!sambaPasswordOriginal) return;
  save(new Blob([JSON.stringify(sambaPasswordOriginal,null,2)],{type:"application/json"}),`zxs-samba-reset-${sambaPasswordOriginal.request_digest}.json`);
});
byId("samba-password-form").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping || !sambaPasswordPreview) return;
  if (byId("samba-new-password").value !== byId("samba-repeat-password").value) { error("两次 Samba 新密码不一致。"); return; }
  const name = sambaPasswordPreview.input.deployment.smb.account;
  if (!window.confirm(`确认真实重置本地 Samba 账户 ${name} 的密码？不创建或启用账户，不更改 ZXS/Unix 密码，不启动服务。结果不明时只查询原请求，不自动重放。`)) return;
  const body = {...sambaPasswordPreview,confirmation:"reset-existing-local-samba-password",
    new_password:byId("samba-new-password").value,repeat_password:byId("samba-repeat-password").value};
  sambaPasswordPreview = null; clearSambaPasswords(); byId("samba-password-form").hidden = true;
  error(""); setBusy(true);
  try {
    const receipt = await (await post("/api/credentials/samba-reset",body)).json();
    byId("samba-reset-receipt").textContent = JSON.stringify(receipt,null,2);
    byId("samba-reset-receipt").hidden = false;
  } catch (failure) { error(`${failure.message}。密码可能已改变。保留原摘要并查询结果，不重复提交原操作，也不要恢复旧 passdb 覆盖其他账户。`); }
  finally { body.new_password = ""; body.repeat_password = ""; clearSambaPasswords(); setBusy(false); }
});
byId("samba-reset-status-form").addEventListener("submit",async event => {
  event.preventDefault(); if (busy || stopping) return;
  const request_digest = byId("samba-reset-digest").value;
  if (!/^[0-9a-f]{64}$/.test(request_digest)) { error("请输入原 Samba 操作的完整摘要。"); return; }
  error(""); setBusy(true);
  try {
    const result = await (await post("/api/credentials/samba-status",{request_digest})).json();
    byId("samba-reset-receipt").textContent = JSON.stringify(result,null,2); byId("samba-reset-receipt").hidden = false;
  } catch (failure) { error(failure.message); } finally { setBusy(false); }
});
window.addEventListener("pagehide",clearSambaPasswords);

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
