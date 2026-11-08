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
let sambaPasswordPreview = null;
let sambaPasswordOriginal = null;
const maxSelectionBytes = 60 * 1024;
function error(message) { byId("error").textContent = message; byId("error").hidden = !message; }
function setBusy(value) { busy = value; for (const form of ["samba-reset-form","samba-password-form","samba-reset-status-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const form of ["first-storage-form", "first-storage-commit-form"]) for (const input of byId(form).elements) input.disabled = value || stopping || !csrf; for (const input of byId("first-administrator-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("first-administrator-password-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("administrator-reset-form").elements) input.disabled = value || stopping || !csrf; for (const input of byId("administrator-password-form").elements) input.disabled = value || stopping || !csrf; byId("check").disabled = value || stopping || !csrf; byId("selection").disabled = value || stopping; byId("selection-file").disabled = value || stopping; byId("install-preview").disabled = value || stopping || !csrf; byId("install-commit").disabled = value || stopping || !csrf; for (const input of byId("deployment-form").elements) input.disabled = value || stopping || !csrf; byId("deployment-prepare").disabled = value || stopping || !csrf; byId("configuration-install").disabled = value || stopping || !csrf; byId("external-administration-stopped").disabled = value || stopping; byId("host-preview").disabled = value || stopping || !csrf; byId("host-prepare").disabled = value || stopping || !csrf; for (const input of byId("layout-form").elements) input.disabled = value || stopping || !csrf; byId("layout-prepare").disabled = value || stopping || !csrf; }
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
