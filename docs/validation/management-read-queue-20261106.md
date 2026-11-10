# 管理 GET 排队修复与验证边界

基线 `4b58c67f50b62808099a8d768330bd4e92bac4ed`，与 CI 头 `b86f935fd7480476f7e0431d0418505f8eff8cc9` 同 tree `beedc5809a519551af21e29809ad13cbf2b7e6f9`。原 CI 在真实 Chrome 的明／暗 Select 快滚通过后，最终网络检查报告 `/api/admin/storage/discovered-disks` HTTP 429。网络日志已被该检查消费，不能从空的事后日志推断无错误。

## 改动

- `api/read_queue.rs` 只调度当前文档的管理 GET，最多一个在途、32 个等待，FIFO、无重试。多级存储刷新共用此队列，不改服务端 `management_slots = 2`，不新增 429 白名单。
- 私有 Permit 随 `Fetched` 保留到正文及 JSON 解码完成或失败；队列等待纳入既有 120 秒 timeout。取消排队、任务 drop 和凭据失效分别释放自身等待记录，不以重置 active 标志回收其他 owner 的许可。
- 发送前再次核对捕获的凭据代次，防排队后同源 cookie 已换会话。旧 401 仅在仍属当前凭据时撤销凭据和等待队列；原 UI epoch／generation 校验不变。
- 本地队列满保留 `status = 0 / RESOURCE_LIMIT`，远端 429 不改码、不改详情或 request ID，也不会自动重发。POST、认证和普通文件读取不排队。
- 队列只持有至多 32 个身份／waker，查找、移除最坏扫描 32 项；无新增锁、网络请求、服务端同步或依赖。既有 Request／正文上限保持。

## 已执行的轻量验证

- 固定 Rust 1.99.0，真实生产 FIFO 原生 15 项通过，聚焦 Clippy `-D warnings -D clippy::all` 通过。覆盖八种真实管理刷新路径、七项突发的 FIFO／单在途、首个／中间等待取消、drop、满队列、失效唤醒、旧 owner 与新许可隔离、waker 更换及失败不重试。正文占用单测只验证 Permit 生命周期，不是实际 fetch 验收。
- 9 项 Node host 观测器用例通过，包括真实标准 Request／Response／ReadableStream 对象、原样字节、429 不被篡改、刻意峰值 2、body 失败／取消、清理、精确请求 timer 和较新无关 timer 的负例，以及旧请求迟到和 cookie 掩盖缺失 Bearer 的负例。该脚本接入正式 Cargo `browser-smoke` 单测；host 通过不代表生产 WASM 通过。
- 精确基线 contract 小库、完整 Web 生产 metadata、完整 Web 测试源码 metadata 和 Clippy metadata 已检查。新增三项 API 凭据／队列／429组合回归仅包含在类型检查中，未链接运行完整 Web 测试二进制。
- 修改文件 rustfmt／diff、第一方架构、维护文档和正式来源核验分别执行并保留日志。无 Cargo manifest、lock 或服务端改动；不更新不可变来源收据。

编译串行，地址空间上限 2 GiB、CPU 上限 180 秒；启动前检查可用内存 2 GiB、`/tmp` 空闲 1 GiB、工作盘空闲 4 GiB。输出仅放 `/tmp`，复用真实依赖；没有 Engine／Server 构建或完整 Web 链接。本轮精确命令、输入哈希、退出码及补丁在独立证据包保存。

## 已接线但未运行

现有真实 browser-smoke 已加入正文持许可、准确排队 timeout、真实退出／新登录、headers 前挂起旧 401、保留新 Bearer、明确一次 429 及显式刷新恢复的场景。当前环境没有本候选的生产 WASM／完整 runner 产物，没有为此启动大型构建；不能宣称修复后的 Chrome、完整 browser-smoke、真实多标签并发或 SMB 已通过。浏览器 runner 的完整 Rust 编译也待正式构建执行。

同一文档 GET 排队不能消除多标签、POST 或已取消客户端背后仍运行的 `HttpBackendJob` 的占槽。取消本地许可不证明服务端工作已结束，真实 429 必须继续可见。只有本来持有 `ReadCancellation` 的请求才支持对应页面 owner 的取消；本次没有给所有管理页面新增导航取消句柄。

本次没有推送、部署、降低后端限额或操作真实磁盘。
