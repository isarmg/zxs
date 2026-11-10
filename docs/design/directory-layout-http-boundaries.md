# 目录实体布局的 HTTP 与 Web 边界

## 派生维护而非业务提交

`POST /api/repositories/{repo}/directory-layout/reconcile` 经[专用 HTTP 适配](../../src/server/src/http_directory_layout.rs)调用[Engine 执行器](../../src/engine/src/engine/directory_layout.rs)。它按当前已发布 State 整理派生布局，不推进 State 序号、不创建目录提交，也不登记可查询的 `OperationResult`。不得生成假 operation ID 或套用通用操作轮询。请求与响应的必需字段和十进制数值规则见[契约](../contracts/README.md#显式实体布局整理)。

## 范围与当前状态

作用域是外层 repository 下全部已显式初始化的目录森林，包括各独立子仓库，不是当前浏览文件夹。稳定 main 的执行时最新保存工作树（包括未提交保存）决定布局，不以当前 checkout 或父级历史 pin 覆盖子仓库 main。`expected_namespace_head` 只防止外层 namespace 已过时，不冻结各独立 main 的精确预览。需要批准一份被冻结的预览时，应另行建立完整版本约束，不能把本接口当作该能力。

Web 从已初始化目录或其普通后代进入明确范围检查，用户再次确认才执行；读取、刷新、初始化均不自动整理。成功返回 state_sequence 是执行基线，不是新增业务提交号。结构性目录/容器计数不是正文完整性、备份份数或安装恢复证明。响应专门提供不可用历史及容器总数，不提供或暗示一份被截短的完整身份列表；非零必须显示部分可用。离线历史不保证已持久启用自动整理，成员重新准入后需显式重新检查。

## 授权与任务生命周期

沿用 protected route、Cookie 同源/CSRF、backend_slots 及 HttpBackendJob。EngineBackend 在同一 writer 锁内重新准入当前管理员、核对仓库与 namespace head，并执行实际整理。任务及 permit 不随 HTTP waiter 取消而提前释放；物理执行没有中途取消按钮。

响应丢失或解码失败只表示结果未知，不能宣称未执行、保存成功或已提交。Web 不自动重试，保留错误码及未知结果说明。不确定结果或 `RecoveryRequired` 会立即使客户端缓存的存储可写状态失效；已在途的健康读取仍占有原槽，但其旧响应不能重新开放写入，结束后重新读取管理状态。

显式恢复按钮先读取新的管理状态，仅在存储可读且会话、当前页面和目录代数仍一致时刷新目录；它不重新提交整理或自动重试打开存储。管理健康仍是服务端的有限新鲜度观察，不是原整理的执行回执，最终写入准入仍由 Engine 决定。解决恢复要求后，用户再明确检查当前范围。会话 epoch 或页面 generation 改变后，旧响应不得覆盖新页面；旧会话不得释放新会话的写入门禁。

## 拒绝与恢复

预检冲突不改变用户 State 或操作记录，也不把未执行整理包装成已发布操作；应用过程中失败则遵循 Engine 写入围栏与恢复要求。`ContainerLayoutInvalid`、`RecoveryRequired`、`NoSpace` 保留各自边界错误。旧业务 physical_path 不等于 backing tree 的实际位置，更不是任意文件系统写入权限。

[真实 Engine 边界回归](../../test/engine/integration/directory_layout_recovery_boundaries.rs)覆盖保留未决决定、缺失定位记录、未知占用和非 metadata primary 离线；[HTTP 回归](../../test/server/integration/http_engine/directory_layout.rs)覆盖实际整理、冷读、认证和陈旧/未知输入拒绝；[Web 回归](../../test/web/unit/files/directory_layout_tests.rs)覆盖重复点击、取消、切页、会话和 signal 卸载。测试源码登记不等于已经运行，具体命令与结果在独立验收证据中记录。此接口不扩展旧格式兼容、外来盘 inode 重绑定或真实设备掉电验收。
