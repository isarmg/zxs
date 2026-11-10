# 保留浏览器探针的使用约束

[test/evidence/ 清单](../../test/evidence/probes.json)登记保留的探针与数学参考。它们不是当前生产验收套件；可能需要匹配的旧夹具或独立构建。缺少输入时不得直接对现有服务运行，也不能把静态检查当作运行通过。当前生产回归使用[browser-smoke](../tools/browser-smoke.md)。

CI 执行 `node .github/scripts/check-evidence-probes.mjs` 核对语法、清单和安全配置，不运行浏览器或访问服务。G4 参考数学与源码保持原样。

## 显式配置与安全重跑

运行这些探针都必须指定：

- `ZXS_PLAYWRIGHT_MODULE`：已经准备好的 Playwright 模块的绝对文件路径、file URL，或可从辅助模块解析的包名；不再引用个人目录或另一个私有项目的 node_modules。
- `ZXS_EVIDENCE_OUTPUT_DIR`：归档目录之外的输出父目录。每次自动建立唯一子目录并打印路径，不覆盖历史证据或之前的重跑结果。
- `ZXS_PRODUCT_ROOT`：可选，匹配的源码/构建仓库根目录；默认由公共辅助模块自身位置推导，不受启动目录或脚本嵌套层数影响。

使用服务的探针还必须设置 `ZXS_EVIDENCE_FIXTURE_URL` 和 `ZXS_EVIDENCE_DISPOSABLE=1`。
URL 仅允许显式端口的 HTTP 回环地址 `127.0.0.1` 或 `[::1]`，不允许凭据、路径、查询或片段。
该确认意味着调用者已独立启动一次性夹具；**不要指向正在使用的业务预览，即使它位于回环地址**。
探针可能登录、修改业务数据及退出会话。代码不会替调用者创建或验证服务的数据隔离。
旧 `ZXS_PREVIEW_URL`、`ZXS_TREE_BASE` 等隐式入口和分散的输出变量不再使用。

额外输入按脚本需要显式提供：

- 账号/维护性服务比较：`ZXS_BROWSER_SEED`、`ZXS_BROWSER_PASSWORD_FILE`。
- version-tree：`ZXS_TREE_SEED`，密码文件由 `ZXS_TREE_PASSWORD_FILE` 或该显式 seed 的 `password_file` 指定。
- merge-tags-backup 本地 WASM：`ZXS_BROWSER_DIST`；served-browser-check：`ZXS_EVIDENCE_RECEIPT`。
- AnimatedShow：`ZXS_ANIMATED_FIXTURE`。

这些变量不得存储密码正文，只指定受保护的输入文件。不要提交凭据或运行生成物。
辅助配置的测试只使用临时目录和无网络配置，不导入或执行历史脚本。
