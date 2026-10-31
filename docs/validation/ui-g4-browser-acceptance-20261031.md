# UI/G4 与浏览器消费者验收

日期：2026-10-31 UTC。基线 main 为 `d3eefd289e53a2803703870fe4250d08cadff9b5`，tree `52d5076189204952dbfaf4d3c2c76b93553d1b3e`。本记录复用精确提交的已执行证据，并单列新增 witness 的状态。

## 已执行矩阵

[完整 CI 41154826502](https://github.com/isarmg/zxs/actions/runs/41154826502) 的 17/17 job 均成功。浏览器环境是 Ubuntu 24.04.5、Linux x86_64、Chrome 与 ChromeDriver `156.0.8078.47`，生产 Web/WASM 来自同次构建并从发行归档解出。

| 消费者／场景 | 实际证据 | 验收边界 |
| --- | --- | --- |
| 文件、元数据、会话及网络 | [Chrome job](https://github.com/isarmg/zxs/actions/runs/41154826502/job/124401633255)，01:40:20–01:41:05 PASS | 真实临时服务、单管理员、loopback HTTP；正文入口缺失及原操作恢复一并检查 |
| G4 与键盘焦点 | 同 job，01:41:02 和 01:41:20 PASS | 明暗模式；真实 CSS 依赖、继承／方向／容器、过渡、作者输出修复、移出／返回及停止清理；计数不是吞吐或布局耗时结论 |
| 字体及必需 CSS | 同 job，01:41:10–01:41:24 PASS | 原字体延迟／404／关闭后交付；CSS 缓存两态、load/error、终态失败和真实重载按钮 |
| 文档／会话中断 | 同 job，01:41:03–01:41:09 PASS | 退出传输失败重试、旧响应隔离、cached/unload/failed 注入、重复恢复；不证明真实 BFCache 命中率 |
| 有界目录与历史消费者 | 同 job，01:40:25、01:40:56 PASS | 合成 205 项目录和 32 节点窗口检查真实 WASM／DOM 上界、驱逐、选择、重审和取消；不是引擎规模证据 |
| 管理与初始化恢复 | 同 job，01:41:25–01:41:34 PASS | 损坏池仍可登录／监测；预设变更 409 后重审；真实预先持久化的权威发布前中断证明补全 |
| CSR 模板消费者 | 同 job，01:41:36 PASS | 真实 DOM/SVG、属性、事件、重建、节点／焦点保留及卸载 |
| WASM 异步消费者 | [foundation-language job](https://github.com/isarmg/zxs/actions/runs/41154826502/job/124400029829)，01:31:29：9 项实际 Chromium runtime checks PASS | 包含 task-dispatch；不是仅编译 |
| 六种响应式尺寸 | Chrome job，01:40:41 PASS；[原始预览 artifact](https://github.com/isarmg/zxs/actions/runs/41154826502/artifacts/13024124994) | 1366×900、1440×960、1512×982、412×915、390×844、1024×1366；固定 DPR 1、`mobile:false`、真实桌面 UA |

失败：上述基线 job 无失败。`41158008645` 属历史生命周期边界运行，不计入浏览器或 G4 通过数。

## 浏览器声明与资源策略

仓库只声明[现代浏览器能力契约](../components/interface.md#modern-browser-contract)，没有 Firefox、Safari、Edge 的品牌／版本兜底承诺。本表只登记实际执行的 Chrome 组合；不将 CSS 视口命名转换为 Android、iOS、iPadOS 或其它引擎通过。

当前资源行为由 [生成器](../../tools/web-build/src/bootstrap.rs) 和静态服务共同实现：

- HTML、`boot.js`、JS/WASM 与 CSS 使用固定 URL；`build-manifest.json` 用于构建／发行校验，浏览器不消费它做发行版身份检查。
- `no-cache` 与内容 ETag 只在重新请求时参与验证。服务启动时建立资源表；更新资源后需要重启服务，不提供旧资源留存仓、旧 wire 兼容层或常驻版本缓存。
- 已挂载／BFCache 保留页不会自动升级。启动资源失败后进入终态，只有用户手动重载；不能把同版缓存故障恢复称为跨发行版旧页自动失效通过。
- 有未知写入或未完成草稿时，现有重载围栏保护内存恢复上下文；本批不引入无条件自动刷新，不自动重发写入，也不更换原操作 ID。

## 新增可执行 witness

以下场景已在[正式 Chrome job 124485872867](https://github.com/isarmg/zxs/actions/runs/41183041841/job/124485872867)实际执行。2026-10-31 UTC，精确候选 `0787d77cf8fa074f6d268bbf05444e54cc07ca52`、tree `a1819b18ccb1a1362cb4a53ae7b5c4729bd63a8d`；逐条读取六个 PASS marker，不以完整 CI 状态代替场景证据。

| 场景 | 源码与要求 | 当前状态 |
| --- | --- | --- |
| 初始化成功回执丢失后防重复 | `runner/management.rs`：真实成功响应丢失；原表单重复 submit、清空输入与真实状态重查均保留 unknown 围栏；只发一次 setup POST；原账户登录及 Config／权威字节不变 | 已运行：08:23:37.545 UTC PASS |
| JS/WASM 故障及迟到初始化 | `runner/stylesheets.rs`：必需运行资源阻断后手动重载；原 WASM Response 晚于非缓存离开或真实启动 deadline 交付，不能重新挂载 | 已运行：08:22:58.233／58.905（JS／WASM 阻断）；08:22:59.142／59.907（两次 pagehide 迟到）；08:23:30.522（自然 30 秒 deadline 后迟到）均 PASS |

上述是两类场景的六次真实 Chrome witness，不是源码、格式或单测推定。迟到 WASM 日志均记录原 Response 的 `status=200`、`mime=application/wasm`、`sameResponse=true`、`module=true`、`instance=true` 且 `completedPhase=failed`，证明真实实例化完成也不能越过终态重挂载。测试交付 gate 不改原 Response 字节，资源阻断也不冒称真实服务端 404、跨发行版升级或网络性能证明。

原开发批次仅执行 Rust 1.99 格式、差异检查及独立边界审阅；本节随后补记上述精确候选的真实运行结果。只复用既有日志，没有为文档登记重跑浏览器，也不外推移动设备、其它浏览器内核、HTTPS 部署或旧资源留存。

## 发布环境待验收清单

- 登记实际发布目标的移动设备、OS／浏览器版本、CPU、DPR、输入法和触控环境，按 [A01](../22-acceptance.md#case-a01) 用同一构建完成文件浏览、元数据、版本树、会话、断网及入口缺失检查。当前 GitHub 桌面 Chrome 的尺寸预览不能替代该设备记录。
- 使用已配置的真实 HTTPS 入口登记证书链、同源代理路径与 Cookie／CSRF 行为，再对桌面及移动发布目标执行同一验收。当前 runner 的临时服务是 loopback HTTP；未提供部署入口，本批不新增部署、证书信任或凭据／权限变更。具备既有目标后可自动执行的部分仍应自动化，不因使用浏览器就归为人工。

G4 不另增 forced-colors／high-contrast 专项要求；物理磁盘、SMB 编辑器及设备掉电验收属于各自独立矩阵，不以本表或截图代替。
