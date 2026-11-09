"use strict";
(() => {
  const prefix = "zxs-helper:vfs-install:";
  const element = name => byId("vfs-install-" + name);
  let reviewed = null;
  const valid = digest => typeof digest === "string" && /^[0-9a-f]{64}$/.test(digest);
  function update() {
    const locked = busy || stopping || !csrf;
    for (const name of ["preview", "status", "original", "ack"]) element(name).disabled = locked;
    element("commit").disabled = locked || reviewed === null || !element("ack").checked;
    element("resume").disabled = locked || !valid(element("original").value) || !element("ack").checked;
  }
  function report(value) {
    element("report").textContent = JSON.stringify(value, null, 2);
  }
  function original() {
    const digest = element("original").value;
    if (!valid(digest)) throw new Error("请输入原 VFS 安装请求的完整摘要。");
    return digest;
  }
  function preserve(digest) {
    const key = prefix + digest;
    const previous = localStorage.getItem(key);
    if (previous !== null && previous !== digest) throw new Error("本机原 VFS 请求记录冲突，未发送安装。");
    localStorage.setItem(key, digest);
    localStorage.setItem(prefix + "last", digest);
    if (localStorage.getItem(key) !== digest) throw new Error("无法核回原 VFS 请求，未发送安装。");
  }
  async function install(digest) {
    if (!element("ack").checked) throw new Error("须持续停止 Samba 与外部软件包／服务管理者。");
    if (!window.confirm("确认只安装同版 helper 内置的离线 ZXS VFS 到固定系统路径？不会联网、替换不同模块、安装 Samba 软件包、改密或启停服务。须持续停止全部外部管理者；未知结果保留原请求，只显式续作同一摘要。")) return;
    preserve(digest);
    element("original").value = digest;
    setBusy(true); error("");
    try {
      const value = await (await post("/api/install/vfs-install", {
        request_digest:digest, confirmation:"install-sealed-samba-vfs-only",
        external_administration_stopped:true
      })).json();
      if (value.request_digest !== digest || value.state !== "vfs_module_installed_not_activated") {
        throw new Error("VFS 响应不对应原安装，不能据此声称完成。");
      }
      report(value); reviewed = null;
    } catch (failure) {
      error(failure.message + "。固定模块可能已发布，保留原摘要和全部文件；不得换载荷、覆盖、回滚或自动重试。");
    } finally { setBusy(false); update(); }
  }
  element("preview").addEventListener("click", async () => {
    if (busy || stopping) return;
    reviewed = null; setBusy(true); error("");
    try {
      const value = await (await post("/api/install/vfs-preview", {})).json();
      if (!valid(value.request_digest)) throw new Error("VFS 预览缺少准确请求摘要。");
      reviewed = value.request_digest;
      element("original").value = reviewed;
      report(value);
    } catch (failure) { error(failure.message); }
    finally { setBusy(false); update(); }
  });
  element("commit").addEventListener("click", async () => {
    if (busy || stopping || reviewed === null) return;
    try { await install(reviewed); } catch (failure) { error(failure.message); }
  });
  element("status").addEventListener("click", async () => {
    if (busy || stopping) return;
    let digest;
    try { digest = original(); } catch (failure) { error(failure.message); return; }
    setBusy(true); error("");
    try {
      const value = await (await post("/api/install/vfs-status", {request_digest:digest})).json();
      if (value.request_digest !== digest) throw new Error("VFS 状态不对应原请求。");
      report(value);
    } catch (failure) { error(failure.message); }
    finally { setBusy(false); update(); }
  });
  element("resume").addEventListener("click", async () => {
    if (busy || stopping) return;
    try { await install(original()); } catch (failure) { error(failure.message); }
  });
  for (const name of ["ack", "original"]) element(name).addEventListener("input", update);
  document.addEventListener("zxs-helper-busy-changed", update);
  try {
    const digest = localStorage.getItem(prefix + "last");
    if (digest !== null) {
      if (!valid(digest)) throw new Error("本机原 VFS 请求索引无效，未重放任何操作。");
      element("original").value = digest;
    }
  } catch (failure) { error(failure.message); }
  update();
})();
