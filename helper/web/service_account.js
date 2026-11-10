"use strict";
(() => {
  const key = "zxs-helper:local-service-account";
  const element = name => byId("service-account-" + name);
  let original = null, preview = null, blocked = false, epoch = 0;
  const same = (left, right) => JSON.stringify(left) === JSON.stringify(right);
  const show = value => { element("report").textContent = JSON.stringify(value, null, 2); };
  function checked(value) {
    if (!value || Object.keys(value).sort().join(",") !== "input,request_digest"
      || !/^[0-9a-f]{64}$/.test(value.request_digest)
      || !value.input || Object.keys(value.input).sort().join(",") !== "service_group,service_user"
      || typeof value.input.service_user !== "string" || typeof value.input.service_group !== "string") {
      throw new Error("服务账户原请求格式无效；未覆盖保存内容。");
    }
    return value;
  }
  function input() {
    return {service_user:element("user").value, service_group:element("group").value};
  }
  function render() {
    const locked = busy || stopping || !csrf || blocked;
    element("user").disabled = locked || original !== null;
    element("group").disabled = locked || original !== null;
    element("preview").disabled = locked;
    element("prepare").disabled = locked || preview === null;
    element("ack").disabled = locked;
    if (original) {
      element("user").value = original.input.service_user;
      element("group").value = original.input.service_group;
    }
  }
  for (const name of ["user", "group"]) element(name).addEventListener("input", () => {
    ++epoch; preview = null; render();
  });
  element("preview").addEventListener("click", async () => {
    if (busy || stopping || !csrf || blocked) return;
    const request = original ? original.input : input(), token = ++epoch;
    preview = null; error(""); setBusy(true);
    try {
      const value = await (await post("/api/initialization/account-preview", request)).json();
      if (token !== epoch) return;
      const selected = checked({input:value.plan.input, request_digest:value.request_digest});
      if (!same(selected.input, request) || (original && !same(original, selected))) {
        throw new Error("账户预览与保留的原请求不同；没有换账户或执行命令。");
      }
      preview = value; show(value);
    } catch (failure) { error(failure.message); }
    finally { setBusy(false); render(); }
  });
  element("prepare").addEventListener("click", async () => {
    if (busy || stopping || !csrf || blocked || !preview) return;
    if (!element("ack").checked) { error("先停止外部账户管理员及其他安装操作。"); return; }
    const request = checked({input:preview.plan.input, request_digest:preview.request_digest});
    if (!window.confirm("确认准备本机服务账户 " + request.input.service_user
      + " 和主组 " + request.input.service_group + "？\n"
      + (preview.requires_creation ? "将通过 systemd-sysusers 创建非登录账户和同名组，不创建 home，不设置登录密码。"
        : "只核对原有 UID/GID，不修改现有账户。")
      + "\n不启动服务。结果未知时先核对，再明确续作同一请求。\n原摘要：" + request.request_digest)) return;
    const token = ++epoch;
    try {
      const stored = sessionStorage.getItem(key);
      if (stored !== (original ? JSON.stringify(original) : null)) throw new Error("账户原请求已经改变；未覆盖它。");
      const text = JSON.stringify(request);
      sessionStorage.setItem(key, text);
      if (sessionStorage.getItem(key) !== text) throw new Error("账户原请求未能准确保存；尚未发送命令。");
      original = request; preview = null; error(""); setBusy(true); render();
      const value = await (await post("/api/initialization/account-prepare", {...request,
        confirmation:"prepare-local-service-account", external_administration_stopped:true})).json();
      if (token !== epoch) return;
      show(value);
      if (value.request_digest !== request.request_digest || value.state !== "local_service_identity_prepared") {
        throw new Error("未收到原账户请求的完成回执；先核对当前状态。");
      }
      element("ack").checked = false;
    } catch (failure) { error(failure.message + " 保留原请求和本机材料，先只读核对，不换账户或自动重放。"); }
    finally { setBusy(false); render(); }
  });
  document.addEventListener("zxs-helper-busy-changed", render);
  window.addEventListener("pagehide", () => { ++epoch; preview = null; });
  window.addEventListener("pageshow", render);
  try {
    const stored = sessionStorage.getItem(key);
    if (stored !== null) {
      if (new TextEncoder().encode(stored).length > 8192) throw new Error("账户原请求超过保存上限。");
      original = checked(JSON.parse(stored));
      show({original, next:"只读核对原账户；没有自动发送命令。"});
    }
  } catch (failure) { blocked = true; error(failure.message); }
  render();
})();
