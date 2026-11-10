"use strict";
(() => {
  const key = "zxs-helper:first-install-tab";
  const maxBytes = 256 * 1024;
  const names = ["安装根", "内置程序", "私有数据布局", "运行目录", "固定服务配置",
    "首次管理账户", "首批空目录存储", "启动与正常管理准入"];
  const states = [
    ["new_host_root_prepared", "existing_host_root_durably_verified"],
    ["new_program_installed", "matching_program_durably_verified"],
    ["new_empty_private_layout_prepared", "matching_empty_private_layout_durably_verified"],
    ["empty_runtime_directories_durably_prepared_not_started"],
    ["configuration_files_durably_installed_not_activated"],
    ["first_management_administrator_durably_created", "original_first_management_administrator_durably_resolved"],
  ];
  let record = null;
  let inspected = false;
  let epoch = 0;
  let blocked = false;
  let liveReady = false;
  let directoryOriginal = null;
  let directoryInspected = false;
  const directoryKey = key + ":directories";
  const element = name => byId("first-install-" + name);
  const show = value => { element("report").textContent = JSON.stringify(value, null, 2); };
  const clearPasswords = () => {
    element("password").value = "";
    element("repeat-password").value = "";
  };
  const same = (left, right) => JSON.stringify(left) === JSON.stringify(right);
  function shape(value, keys) {
    if (!value || typeof value !== "object" || Array.isArray(value)
      || Object.keys(value).sort().join(",") !== keys.slice().sort().join(",")) {
      throw new Error("首装原请求格式不符；未覆盖保存内容。");
    }
  }
  function checked(value) {
    shape(value, ["format", "revision", "input", "steps"]);
    if (value.format !== "zxs-helper-first-install-tab" || value.revision !== 1) {
      throw new Error("不支持这个首装原请求格式；没有转换或重置。");
    }
    const input = value.input;
    shape(input, ["deployment", "administrator", "roots", "enable_on_boot"]);
    shape(input.deployment, ["service_user", "service_group", "listen", "smb"]);
    for (const text of [input.deployment.service_user, input.deployment.service_group,
      input.deployment.listen, input.administrator]) {
      if (typeof text !== "string" || !text.length || text.length > 256) {
        throw new Error("首装账户或监听输入无效。");
      }
    }
    if (typeof input.enable_on_boot !== "boolean" || !Array.isArray(input.roots)
      || !input.roots.length || input.roots.length > 32
      || input.roots.some(path => typeof path !== "string" || !path.length || path.length > 4096)) {
      throw new Error("首装完整目录批次或开机启动选择无效。");
    }
    if (input.deployment.smb !== null) {
      shape(input.deployment.smb, ["account", "peer_uid"]);
      const smb = input.deployment.smb;
      if (typeof smb.account !== "string" || !smb.account.length || smb.account.length > 32
        || !Number.isInteger(smb.peer_uid) || smb.peer_uid < 0 || smb.peer_uid > 4294967295) {
        throw new Error("首装 Samba 选择无效。");
      }
    }
    if (!Array.isArray(value.steps) || value.steps.length > names.length) {
      throw new Error("首装原步骤数量无效。");
    }
    value.steps.forEach((step, index) => {
      shape(step, ["request_digest", "plan", "issued", "completed"]);
      if (!/^[0-9a-f]{64}$/.test(step.request_digest)
        || typeof step.issued !== "boolean" || typeof step.completed !== "boolean"
        || (step.completed && !step.issued)
        || (index < value.steps.length - 1 && !step.completed)
        || (index === 6 ? !step.plan || typeof step.plan !== "object" || Array.isArray(step.plan) : step.plan !== null)) {
        throw new Error("首装原步骤或摘要不符；未重新选择目标。");
      }
    });
    return value;
  }
  function encoded(value) {
    const text = JSON.stringify(checked(value));
    if (new TextEncoder().encode(text).length > maxBytes) throw new Error("首装原请求超过 256 KiB 保存上限。");
    return text;
  }
  function persist(next) {
    const text = encoded(next);
    const current = sessionStorage.getItem(key);
    if (record !== null && current !== encoded(record)) {
      throw new Error("本标签页原请求已改变；停止发送，不覆盖它。");
    }
    if (record === null && current !== null) throw new Error("已有首装原请求，须先恢复它。");
    sessionStorage.setItem(key, text);
    if (sessionStorage.getItem(key) !== text) throw new Error("首装原请求未准确保存，尚未发送写入。");
    record = next;
  }
  function index() {
    const pending = record.steps.findIndex(step => !step.completed);
    return pending >= 0 ? pending : Math.min(record.steps.length, names.length - 1);
  }
  function requestInput(step) {
    const input = record.input;
    switch (step) {
      case 0: case 1: return {};
      case 2: case 3: return {service_user:input.deployment.service_user};
      case 4: return input.deployment;
      case 5: return {service_user:input.deployment.service_user, administrator:input.administrator};
      case 6: return {service_user:input.deployment.service_user, roots:input.roots};
      case 7: return {deployment:input.deployment, enable_on_boot:input.enable_on_boot};
      default: throw new Error("无效的首装界面步骤。");
    }
  }
  function previewRequest(step, original) {
    const input = requestInput(step);
    const ordinary = [
      "/api/host/preview", "/api/install/preview", "/api/initialization/layout-preview",
      "/api/initialization/runtime-preview", "/api/deployment/install-preview",
      "/api/initialization/administrator-preview", "/api/initialization/storage-preview",
      "/api/deployment/activation-preview",
    ];
    if (!original?.issued) return [ordinary[step], input];
    const request_digest = original.request_digest;
    switch (step) {
      case 4: return ["/api/deployment/install-recover-preview", {request_digest}];
      case 5: return ["/api/initialization/administrator-recover-preview", {input, request_digest}];
      case 6: return ["/api/initialization/storage-recover-preview", {service_user:input.service_user, request_digest}];
      case 7: return ["/api/deployment/activation-status", {input, request_digest}];
      default: return [ordinary[step], input];
    }
  }
  function commitRequest(step, original) {
    const input = requestInput(step), request_digest = original.request_digest;
    switch (step) {
      case 0: return ["/api/host/prepare", {request_digest, confirmation:"prepare-host-root-only"}];
      case 1: return ["/api/install/commit", {request_digest, confirmation:"install-program-only"}];
      case 2: return ["/api/initialization/layout-prepare", {input, request_digest, confirmation:"prepare-private-layout-only"}];
      case 3: return ["/api/initialization/runtime-prepare", {input, request_digest,
        confirmation:"prepare-empty-runtime-only", external_administration_stopped:true}];
      case 4: return ["/api/deployment/install-files", {input, request_digest,
        confirmation:"install-configuration-files-only", external_administration_stopped:true}];
      case 5: return ["/api/initialization/administrator-create", {input, request_digest,
        confirmation:"initialize-first-management-administrator", external_writers_stopped:true,
        password:element("password").value, repeat_password:element("repeat-password").value}];
      case 6: return ["/api/initialization/storage-create", {
        input:{service_user:input.service_user, request_digest}, plan:original.plan,
        confirmation:"initialize-first-storage-batch", external_writers_stopped:true}];
      case 7: return ["/api/deployment/activate", {input, request_digest,
        confirmation:"activate-installed-zxs-services", external_administration_stopped:true}];
      default: throw new Error("无效的首装界面步骤。");
    }
  }
  function render() {
    const locked = busy || stopping || !csrf || blocked;
    const preparesDirectories = record !== null && index() === 6
      && record.steps[6] === undefined && directoryInspected;
    for (const input of element("form").elements) input.disabled = locked || record !== null;
    element("import").disabled = locked || record !== null;
    element("save").disabled = locked || record === null;
    element("inspect").disabled = locked || record === null;
    element("refresh-preview").disabled = locked || record === null;
    element("refresh-preview").hidden = true;
    byId("samba-return-first-install").disabled = locked || record === null;
    byId("samba-return-first-install").hidden = record === null || record.input.deployment.smb === null;
    element("commit").disabled = locked || record === null || (!inspected && !preparesDirectories);
    element("ack").disabled = locked;
    element("samba-settings").disabled = locked || record === null;
    element("account-settings").hidden = record === null || index() !== 2;
    element("account-settings").disabled = locked;
    element("samba-account-settings").hidden = record === null || record.input.deployment.smb === null
      || index() < 2 || index() > 4;
    element("samba-account-settings").disabled = locked;
    element("directories-save").disabled = locked || directoryOriginal === null;
    element("roots-correct").hidden = record === null || index() !== 6
      || record.steps[6] !== undefined || directoryOriginal !== null;
    element("roots-correct").disabled = locked;
    if (!element("roots-correct").hidden) element("roots").disabled = locked;
    if (record === null) {
      element("progress").textContent = blocked ? "原请求不可读；未清除记录或发送操作。" : "填一次输入，再逐步核对和确认原生操作。";
      element("commit").hidden = true;
      element("passwords").hidden = true;
      return;
    }
    const step = index(), original = record.steps[step];
    element("refresh-preview").hidden = !original || original.issued || original.completed;
    byId("samba-return-first-install").hidden = record.input.deployment.smb === null || step < 4;
    const input = record.input, deployment = input.deployment;
    for (const [name, value] of [
      ["user", deployment.service_user], ["group", deployment.service_group],
      ["listen", deployment.listen], ["administrator", input.administrator],
      ["roots", input.roots.join("\n")], ["smb-account", deployment.smb?.account || ""],
      ["peer-uid", String(deployment.smb?.peer_uid ?? 0)],
    ]) element(name).value = value;
    element("smb").checked = deployment.smb !== null;
    element("autostart").checked = input.enable_on_boot;
    element("directory-note").hidden = step !== 6;
    element("directories-actions").hidden = step !== 6 || original !== undefined;
    element("progress").textContent = "第 " + (step + 1) + "/" + names.length + " 步：" + names[step]
      + (original?.issued && !original.completed ? "。已发送或发送结果未知，只续同一请求。" : "")
      + (original?.completed ? "。历史完成记录已保留，请查询当前服务状态。" : "");
    element("commit").hidden = !preparesDirectories
      && (!original || (step === 7 ? liveReady : original.completed));
    element("commit").textContent = preparesDirectories ? "确认创建原批次空目录"
      : original?.issued ? "再次确认并仅续作这个原步骤" : "确认执行当前原步骤";
    element("passwords").hidden = step !== 5 || original?.completed;
    element("samba-note").hidden = record.input.deployment.smb === null || step < 4;
    for (const input of element("passwords").querySelectorAll("input")) input.disabled = locked;
  }
  function complete(step, value, original) {
    const digest = step >= 3 && step <= 5 ? value.receipt?.request_digest : value.request_digest;
    if (digest !== original.request_digest) throw new Error("响应不属于当前原步骤；结果仍未知。");
    if (step < 6 && !states[step].includes(value.state)) throw new Error("未收到这个原步骤的完成回执。");
    if (step === 6 && (value.format !== "zxs-helper-first-storage-batch-complete"
      || value.storage_activation_completed !== true)) throw new Error("首批存储尚未完成。");
    if (step === 7 && value.state !== "services_active_and_management_ready") return false;
    return true;
  }
  async function inspect() {
    if (busy || stopping || !csrf || blocked || !record) return;
    const token = ++epoch, step = index(), original = record.steps[step];
    inspected = false; directoryInspected = false;
    element("open").hidden = true;
    liveReady = false;
    setBusy(true); error(""); render();
    try {
      if (step === 6 && !original) {
        const prepared = await inspectDirectories(token);
        if (token !== epoch || !prepared) return;
      }
      const [path, input] = previewRequest(step, original);
      const value = await (await post(path, input)).json();
      if (token !== epoch) return;
      if (!/^[0-9a-f]{64}$/.test(value.request_digest)
        || (original && value.request_digest !== original.request_digest)) {
        throw new Error("当前环境与原步骤摘要不同；不替换原请求。");
      }
      if (original && step === 6 && !same(original.plan, value.plan)) {
        throw new Error("当前存储计划与保留的完整原计划不同。");
      }
      if (!original) {
        const next = structuredClone(record);
        next.steps.push({request_digest:value.request_digest, plan:step === 6 ? value.plan : null,
          issued:false, completed:false});
        persist(next);
      }
      show(value);
      inspected = true;
      if (step === 7 && original?.issued) {
        if (complete(step, value, original)) finish(step);
      }
    } catch (failure) {
      inspected = original?.issued === true && (!original.completed || step === 7);
      error(failure.message + (inspected ? " 已保留完整原输入；只允许另行明确重试这个原步骤，不换新请求。" : ""));
    } finally { setBusy(false); render(); }
  }
  function finish(step) {
    const next = structuredClone(record);
    next.steps[step].completed = true;
    persist(next);
    inspected = false;
    element("ack").checked = false;
    if (step === 7) {
      liveReady = true;
      const url = new URL(window.location.href);
      const listen = record.input.deployment.listen;
      const port = listen.slice(listen.lastIndexOf(":") + 1);
      if (!/^[0-9]+$/.test(port) || Number(port) < 1 || Number(port) > 65535) throw new Error("原服务端口无效。");
      url.protocol = "http:"; url.port = port; url.pathname = "/"; url.search = ""; url.hash = "";
      element("web").href = url.toString();
      element("open").hidden = false;
    }
  }
  element("form").addEventListener("submit", event => {
    event.preventDefault();
    if (busy || stopping || !csrf || record !== null || blocked) return;
    try {
      const smb = element("smb").checked ? {
        account:element("smb-account").value, peer_uid:Number(element("peer-uid").value),
      } : null;
      persist({format:"zxs-helper-first-install-tab", revision:1, input:{
        deployment:{service_user:element("user").value, service_group:element("group").value,
          listen:element("listen").value, smb},
        administrator:element("administrator").value,
        roots:element("roots").value.split(/\r?\n/).filter(line => line.length > 0),
        enable_on_boot:element("autostart").checked,
      }, steps:[]});
      show({original_input:record.input, next:"只读核对第一步"});
      render();
    } catch (failure) { error(failure.message); }
  });
  function openAccountCard(prefix, user, group) {
    const fields = [["user", user], ["group", group]].map(([name, value]) => ({
      field:byId(prefix + "-" + name), value,
    }));
    if (fields.some(({field, value}) => field.disabled && field.value !== value)) {
      error("账户卡片保留的是另一原请求，未替换它或修改首装输入。请核对原账户记录。");
      return;
    }
    for (const {field, value} of fields) {
      field.value = value;
      field.dispatchEvent(new Event("input", {bubbles:true}));
    }
    byId(prefix).scrollIntoView({block:"start"});
    error("已带入同一账户，未创建账户。请先只读核对并明确确认，完成后返回当前首装步骤。");
  }
  element("account-settings").addEventListener("click", () => {
    if (busy || stopping || !csrf || blocked || !record || index() !== 2) return;
    const deployment = record.input.deployment;
    openAccountCard("service-account", deployment.service_user, deployment.service_group);
  });
  element("samba-account-settings").addEventListener("click", () => {
    if (busy || stopping || !csrf || blocked || !record || index() < 2 || index() > 4) return;
    const deployment = record.input.deployment;
    if (deployment.smb === null) return;
    if (deployment.smb.account === deployment.service_user) {
      error("Samba Unix 账户必须独立于 ZXS 私有服务账户；没有发送账户命令。");
      return;
    }
    openAccountCard("samba-unix-account", deployment.smb.account, deployment.smb.account);
  });
  element("inspect").addEventListener("click", inspect);
  element("roots-correct").addEventListener("click", () => {
    if (busy || stopping || !csrf || blocked || !record) return;
    try {
      directoryInput();
      if (directoryOriginal !== null || sessionStorage.getItem(directoryKey) !== null) {
        throw new Error("空目录原请求已经保留；只能查询和续作原批次，不能更换目录。");
      }
      const next = structuredClone(record);
      next.input.roots = element("roots").value.split(/\r?\n/).filter(line => line.length > 0);
      if (!window.confirm("修改尚未保留空目录请求的首批目录？\n" + next.input.roots.join("\n")
        + "\n保留此前全部安装步骤、账户与配置；下一次只读核对仍由原生接口决定能否接纳。")) return;
      persist(next);
      ++epoch; inspected = false; directoryInspected = false; liveReady = false;
      element("ack").checked = false;
      show({original_input:record.input, next:"目录已更正，明确点击只读核对；尚未发送系统操作。"});
      error(""); render();
    } catch (failure) { error(failure.message); }
  });
  element("refresh-preview").addEventListener("click", () => {
    if (busy || stopping || !csrf || blocked || !record) return;
    const step = index(), original = record.steps[step];
    if (!original || original.issued || original.completed || step !== record.steps.length - 1) return;
    if (!window.confirm("撤回本标签页尚未发送的第 " + (step + 1) + " 步预览？\n原摘要："
      + original.request_digest + "\n保留完整首装输入和此前全部原步骤，只允许重新只读核对。"
      + "\n不撤销任何磁盘或服务动作；其他标签页及原生保留材料仍须由后端重新准入。")) return;
    try {
      const next = structuredClone(record);
      next.steps.pop();
      persist(next);
      ++epoch; inspected = false; directoryInspected = false; liveReady = false;
      clearPasswords(); element("ack").checked = false; element("open").hidden = true;
      show({original_input:record.input, released_unissued_preview:original.request_digest,
        next:"明确点击只读核对当前原步骤；没有发出或重放任何系统动作。"});
      error(""); render();
    } catch (failure) { error(failure.message); }
  });
  element("commit").addEventListener("click", async () => {
    if (busy || stopping || !csrf || blocked || !record) return;
    const step = index(), original = record.steps[step];
    if (step === 6 && !original && directoryInspected) {
      await prepareDirectories();
      return;
    }
    if (!inspected) return;
    if (!original || (original.completed && step !== 7)) return;
    if (!element("ack").checked) { error("先确认已停止外部写者与其他安装/服务操作。"); return; }
    if (step === 5 && (!element("password").value
      || element("password").value !== element("repeat-password").value)) {
      error("请输入两次相同的初始管理员密码；续作必须是原来的密码。"); return;
    }
    const detail = step === 6 ? "\n全部原目录：\n" + record.input.roots.join("\n")
      : step === 7 ? "\n将启动 ZXS" + (record.input.deployment.smb ? " 和独立 Samba" : "")
        + (record.input.enable_on_boot ? "，就绪后启用开机启动。" : "，不改变开机启动设置。")
      : step === 5 ? "\n只创建原管理员，续作不替换原密码。" : "";
    if (!window.confirm("确认以 root 执行第 " + (step + 1) + " 步「" + names[step]
      + "」？\n原摘要：" + original.request_digest + detail
      + "\n失败可能已生效；不回滚、不清空、不自动重放。")) return;
    const token = ++epoch;
    let body = null;
    try {
      const next = structuredClone(record);
      next.steps[step].issued = true;
      persist(next);
      const request = commitRequest(step, original);
      body = request[1];
      clearPasswords(); inspected = false; liveReady = false;
      element("open").hidden = true;
      error(""); setBusy(true); render();
      const value = await (await post(request[0], body)).json();
      if (token !== epoch) return;
      show(value);
      if (complete(step, value, original)) finish(step);
      else error("服务尚未通过正常管理准入；保留原启动请求，只查询后再明确续作。");
    } catch (failure) {
      error(failure.message + " 原步骤已保留；先查询，不换新输入或清理目录。");
    } finally {
      if (body && step === 5) { body.password = ""; body.repeat_password = ""; }
      clearPasswords(); setBusy(false); render();
    }
  });
  function directoryInput() {
    if (!record || index() !== 6 || record.steps[6] !== undefined) {
      throw new Error("只在首存储计划尚未选择前准备原批次空目录。");
    }
    return {service_user:record.input.deployment.service_user, roots:record.input.roots};
  }
  function checkedDirectoryOriginal(value, input) {
    shape(value, ["input", "request_digest"]);
    if (!same(value.input, input) || !/^[0-9a-f]{64}$/.test(value.request_digest)) {
      throw new Error("空目录原请求与首装完整输入不符；未覆盖它。");
    }
    return value;
  }
  function readDirectoryOriginal(stored, input) {
    if (stored === null) return null;
    if (new TextEncoder().encode(stored).length > 64 * 1024) throw new Error("空目录原请求超过保存上限。");
    return checkedDirectoryOriginal(JSON.parse(stored), input);
  }
  async function inspectDirectories(token) {
    const input = directoryInput();
    const stored = sessionStorage.getItem(directoryKey);
    if (stored !== null) {
      directoryOriginal = readDirectoryOriginal(stored, input);
    }
    const value = await (await post("/api/initialization/storage-directories-preview", input)).json();
    if (token !== epoch) return false;
    const original = checkedDirectoryOriginal({input:value.input, request_digest:value.request_digest}, input);
    if (directoryOriginal && !same(original, directoryOriginal)) {
      throw new Error("原空目录计划或宿主身份改变；未选择替代计划。");
    }
    const text = JSON.stringify(original);
    if (new TextEncoder().encode(text).length > 64 * 1024) throw new Error("空目录原请求超过保存上限。");
    if (sessionStorage.getItem(directoryKey) !== stored) throw new Error("空目录原请求在核对期间改变。");
    sessionStorage.setItem(directoryKey, text);
    if (sessionStorage.getItem(directoryKey) !== text) throw new Error("空目录原请求未能准确保存。");
    directoryOriginal = original;
    const prepared = value.directories.every(directory => directory.state === "empty_prepared");
    directoryInspected = !prepared;
    show(value);
    return prepared;
  }
  async function prepareDirectories() {
    if (busy || stopping || !csrf || blocked || !directoryOriginal || !directoryInspected) return;
    if (!element("ack").checked) { error("先确认已停止外部写者与其他主机管理操作。"); return; }
    const original = directoryOriginal;
    if (!window.confirm("仅准备这批原空目录？\n" + original.input.roots.join("\n")
      + "\n只创建缺少的直接子目录并交给原服务 UID/主 GID；不改已有权限、不删数据、不初始化池、不挂载/格式化。"
      + "\n原摘要：" + original.request_digest + "\n中断后只查询和续作同一批次。")) return;
    const token = ++epoch;
    let completed = false;
    error(""); setBusy(true); directoryInspected = false; render();
    try {
      checkedDirectoryOriginal(original, directoryInput());
      if (sessionStorage.getItem(directoryKey) !== JSON.stringify(original)) {
        throw new Error("原目录请求未保留，尚未发送写入。");
      }
      const value = await (await post("/api/initialization/storage-directories-prepare", {
        ...original, confirmation:"prepare-first-storage-directories-only", external_writers_stopped:true,
      })).json();
      if (token !== epoch) return;
      show(value);
      if (value.request_digest !== original.request_digest
        || value.state !== "original_empty_storage_directories_durably_prepared") {
        throw new Error("未收到原目录批次的完成回执；结果仍未知。");
      }
      element("ack").checked = false;
      completed = true;
    } catch (failure) {
      error(failure.message + " 保留原请求、pending 和全部目录；先查询，不清空、不换批次。");
    } finally { setBusy(false); render(); }
    if (completed && token === epoch) await inspect();
  }
  element("directories-save").addEventListener("click", () => {
    if (busy || stopping || !directoryOriginal) return;
    save(new Blob([JSON.stringify(directoryOriginal)], {type:"application/json"}), "zxs-first-storage-directories-original.json");
  });
  element("samba-settings").addEventListener("click", () => {
    if (busy || stopping || !record?.input.deployment.smb) return;
    const deployment = record.input.deployment;
    for (const [id, value] of [
      ["service-user", deployment.service_user], ["service-group", deployment.service_group],
      ["service-listen", deployment.listen], ["smb-account", deployment.smb.account],
      ["smb-peer-uid", String(deployment.smb.peer_uid)],
    ]) {
      byId(id).value = value;
      byId(id).dispatchEvent(new Event("input", {bubbles:true}));
    }
    byId("smb-enabled").checked = true;
    byId("smb-enabled").dispatchEvent(new Event("input", {bubbles:true}));
    byId("samba-reset-form").scrollIntoView({block:"start"});
    error("已带入同一配置，未执行密码或账户操作。完成独立预览与确认后回到首装向导。");
  });
  byId("samba-return-first-install").addEventListener("click", () => {
    if (busy || stopping || !csrf || blocked || !record?.input.deployment.smb) return;
    try {
      if (!same(sambaDeploymentInput(), record.input.deployment)) {
        throw new Error("Samba 卡片当前配置与首装冻结输入不同；保留两边原操作，不把它当作本次首装的完成。");
      }
      if (sessionStorage.getItem(key) !== encoded(record)) {
        throw new Error("首装原请求已改变；没有覆盖它或继续操作。");
      }
      const original = sambaPasswordOriginal;
      const reference = original && same(original.input?.deployment, record.input.deployment)
        && /^[0-9a-f]{64}$/.test(original.request_digest)
        ? {kind:original.kind, request_digest:original.request_digest} : null;
      invalidateSambaPreview();
      ++epoch; inspected = false; directoryInspected = false; liveReady = false;
      clearPasswords(); element("ack").checked = false; element("open").hidden = true;
      show({original_input:record.input, samba_original_reference:reference,
        next:"已返回同一首装请求。若 Samba 结果未知，仍须在原卡片查询；此处不会重放登记、改密或跳步。明确核对当前原步骤后再继续。"});
      error(""); render();
      byId("first-install").scrollIntoView({block:"start"});
      element("inspect").focus({preventScroll:true});
    } catch (failure) { error(failure.message); }
  });
  element("save").addEventListener("click", () => {
    if (busy || stopping || !record || blocked) return;
    try { save(new Blob([encoded(record)], {type:"application/json"}), "zxs-first-install-original.json"); }
    catch (failure) { error(failure.message); }
  });
  element("import").addEventListener("change", async event => {
    if (busy || stopping || record !== null || blocked) return;
    const file = event.target.files[0];
    if (!file) return;
    try {
      if (file.size > maxBytes) throw new Error("首装原请求文件超过 256 KiB。");
      const token = ++epoch, text = await file.text();
      if (token !== epoch || busy || stopping || record !== null) return;
      persist(checked(JSON.parse(text)));
      inspected = false;
      show({original_input:record.input, note:"导入的是恢复参考，不是当前就绪证明；没有发出操作。"});
      render();
    } catch (failure) { error(failure.message); }
  });
  window.addEventListener("pagehide", () => {
    ++epoch; inspected = false; directoryInspected = false; liveReady = false;
    element("open").hidden = true; clearPasswords();
  });
  window.addEventListener("pageshow", render);
  document.addEventListener("zxs-helper-busy-changed", render);
  try {
    const text = sessionStorage.getItem(key);
    if (text !== null) {
      if (new TextEncoder().encode(text).length > maxBytes) throw new Error("保存的首装原请求过大；未清除。");
      record = checked(JSON.parse(text));
      if (index() === 6 && record.steps[6] === undefined) {
        directoryOriginal = readDirectoryOriginal(sessionStorage.getItem(directoryKey), directoryInput());
      }
      show({original_input:record.input, note:"原请求已恢复；只读核对，不自动重放。"});
    }
  } catch (failure) { blocked = true; error(failure.message); }
  render();
})();
