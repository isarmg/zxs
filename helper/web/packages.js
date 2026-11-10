"use strict";
(() => {
  const element = name => byId("packages-" + name);
  let scope = element("scope").value;
  const key = () => "zxs-helper:system-packages:" + scope;
  let original = null, preview = null, blocked = false, epoch = 0;
  const show = value => { element("report").textContent = JSON.stringify(value, null, 2); };
  function digest(value) {
    if (typeof value !== "string" || !/^[0-9a-f]{64}$/.test(value)) throw new Error("系统包原摘要无效，未覆盖它。");
    return value;
  }
  function render() {
    const locked = busy || stopping || !csrf || blocked;
    element("scope").disabled = locked;
    element("preview").disabled = locked;
    element("refresh").disabled = locked || original !== null
      || preview?.state === "original_pending" || preview?.state === "completed";
    element("install").disabled = locked || original !== null || preview?.state !== "installable";
    element("finish").disabled = locked || preview?.state !== "original_pending" || preview.packages_installed !== true;
    element("ack").disabled = locked;
  }
  element("preview").addEventListener("click", async () => {
    if (busy || stopping || !csrf || blocked) return;
    const token = ++epoch; preview = null; error(""); setBusy(true);
    try {
      const value = await (await post("/api/initialization/packages-preview", {scope})).json();
      if (token !== epoch) return;
      if (value.scope !== scope) throw new Error("响应不属于所选依赖。请重新核对当前选择。");
      if (!["required_packages_installed", "installable", "original_pending", "completed"].includes(value.state)) throw new Error("未知系统包观察，未启用安装。");
      if (value.state !== "required_packages_installed") digest(value.request_digest);
      if (value.state === "installable" && typeof value.plan?.simulation !== "string") throw new Error("安装方案缺少完整 APT 输出。");
      if (value.state === "original_pending" && typeof value.packages_installed !== "boolean") throw new Error("原安装缺少准确包状态。");
      if (original !== null && value.request_digest !== original) throw new Error("观察不属于保留的系统包原请求。");
      preview = value; show(value);
    } catch (failure) { error(failure.message); }
    finally { setBusy(false); render(); }
  });
  for (const action of ["refresh", "install", "finish"]) element(action).addEventListener("click", async () => {
    if (busy || stopping || !csrf || blocked || element(action).disabled) return;
    if (!element("ack").checked) { error("先停止外部包、服务及账户管理员并勾选确认。"); return; }
    const operation = {
      refresh:["refresh-system-package-indexes", "从现有签名 APT 软件源下载/更新索引？不添加软件源、不安装包。"],
      install:["install-system-packages", scope === "samba" ? "确认安装完整 APT 方案中的固定版本 Samba、systemd 及依赖？包脚本启动暂时禁止，安装完成后取消新 Samba 默认单元自启并清理本次策略。" : "确认安装完整 APT 方案中的 systemd 及依赖？本次不请求 Samba 包。包脚本启动暂时禁止，完成后清理本次策略。"],
      finish:["finish-original-system-packages", "确认核对所选原安装并完成对应的服务策略清理？此步骤不重复 APT。"],
    }[action];
    const request = {scope, confirmation:operation[0], external_administration_stopped:true};
    if (action !== "refresh") request.request_digest = digest(preview.request_digest);
    if (!window.confirm(operation[1] + (action === "install" ? "\n" + preview.plan.simulation : ""))) return;
    const token = ++epoch;
    try {
      if (action !== "refresh") {
        const saved = sessionStorage.getItem(key());
        if (saved !== original) throw new Error("系统包原记录发生变化；未替换它。");
        sessionStorage.setItem(key(), request.request_digest);
        if (sessionStorage.getItem(key()) !== request.request_digest) throw new Error("系统包原摘要未保存，未发送操作。");
        original = request.request_digest;
      }
      preview = null; error(""); setBusy(true); render();
      const value = await (await post("/api/initialization/packages-" + action, request)).json();
      if (token !== epoch) return;
      show(value); element("ack").checked = false;
      error("操作已返回；请只读核对当前包状态，再返回原首装步骤。包回执不是 VFS、密码或服务就绪证明。");
    } catch (failure) { error(failure.message + " 保留原请求和包状态，先核对；不重新安装或删除 /usr/sbin/policy-rc.d。"); }
    finally { setBusy(false); render(); }
  });
  document.addEventListener("zxs-helper-busy-changed", render);
  window.addEventListener("pagehide", () => { ++epoch; preview = null; });
  window.addEventListener("pageshow", render);
  function loadOriginal() {
    original = null; preview = null; blocked = false; ++epoch;
    try {
      if (!["systemd", "samba"].includes(scope)) throw new Error("请选择基础工具或 Samba 依赖。");
      const saved = sessionStorage.getItem(key());
      if (saved !== null) original = digest(saved);
      show({scope, request_digest:original, next:"只读核对所选依赖。"});
    } catch (failure) { blocked = true; error(failure.message); }
    element("ack").checked = false;
    render();
  }
  element("scope").addEventListener("input", () => {
    if (busy || stopping) { element("scope").value = scope; return; }
    scope = element("scope").value;
    loadOriginal();
  });
  loadOriginal();
})();
