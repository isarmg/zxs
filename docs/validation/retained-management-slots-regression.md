# 取消 HTTP 等待后仍持有管理槽的真实路由回归

基线为 `4b58c67f50b62808099a8d768330bd4e92bac4ed`。本批在独立分支 `management-retained-slots-regression` 增加测试，未修改 Web worker 的工作树、服务端业务源码、两个管理槽的限额、Cargo manifest 或 lock。

## 覆盖的交错与断言

新测试 [retained_management_slots.rs](../../test/server/unit/retained_management_slots.rs) 接入原有 `backend_admission_tests` 模块，复用原来的阻塞 BackendProbe、真实 AuthStore、真实 router、HttpBackendJob 及 JSON 错误编码。没有手工生成 429 响应。

两个具名用例分别结束 HTTP 等待：

- `two_dropped_http_waits_keep_management_slots_until_backend_completion` 直接丢弃正在等待的路由响应 future。
- `two_timed_out_http_waits_keep_management_slots_until_backend_completion` 用真实 Tokio timeout 结束同一响应 future。超时对象是路由 future，不是只会被分离的 JoinHandle。

每次都先并行轮询响应 future 与后台 started 信号，确认后台实际进入阻塞 gate，才丢弃或让等待超时。因此不是尚未发送的客户端队列取消，也不是未轮询请求产生的空证明。连续两次结束 HTTP 等待后，后台调用数保持 2、完成数保持 0。

随后从真实生产 router 请求 `/api/workspaces/capacity`、`/api/capabilities`、`/readyz`，每条必须得到真实 HTTP 429。响应还经生产共享客户端协议解码，核对：

- 状态仍为 429，错误码仍为 `RESOURCE_LIMIT_EXCEEDED`。
- 原始详情为 `management capacity reached`，retryable 为 true。
- 存在非空、彼此不同的服务端 request ID。
- 所有拒绝均未进入阻塞后端，后台调用数仍为 2、完成数仍为 0。
- 独立 `/healthz` 仍能完成并返回 200。

释放 gate 后先等待两个后台完成信号，再有界观察实际许可回收。完成信号先于 permit Drop，不能用一次 yield 或固定 sleep 冒充许可已经释放。仅此明确释放后的测试等待阶段允许暂态 429；释放前的三次拒绝均被逐项核验，未被重试或忽略。

最后新读取必须实际返回 200，正文可解码为准确 WorkspaceCapacity，后台累计调用数严格为 3，即两次已取消读取加一次成功新读取。所有等待均有 5 秒边界；失败时 Release guard 仍会打开 gate，避免留下阻塞线程。

默认 `core` 特性另核验发现磁盘及 registrations 路由在同一共享预算耗尽时返回 `management operation capacity reached`，并保留准确错误码与 request ID。

## 正式入口

```sh
cargo test --locked --offline -p storage-server --lib retained_management_slots:: -- --test-threads=1
```

两个测试由原包的真实 cfg(test) 入口接线，不增加第二工作区、生产测试 hook 或公开 API。

## 本机实际执行

2026-11-06，Rust 1.99.0，Linux x86_64。

1. 对真实服务端单测目标执行 `--test --emit=metadata`，关闭可选 core 特性，全部类型检查通过。
2. 全量单测二进制在 2 GiB 虚拟地址限制下链接失败；同限制下的生产库编译也因分配失败退出。两份失败日志均保留，没有把类型检查冒充完整执行。
3. 采用单 CPU／单 codegen unit，独立监控编译进程组实际驻留内存上限 2 GiB，同时允许 4 GiB 虚拟地址用于只读依赖映射。未改动的真实服务端生产库编译成功，峰值实际驻留内存 1,998,368,768 字节，虚拟地址峰值 3,317,219,328 字节；没有 Engine 构建或共享缓存写入。
4. 从既有 backend_admission.rs 原样提取首个测试之前的 900 行夹具及准确 probe_capacity、probe_content、private_tempdir 测试辅助函数，链接真实生产服务端库和原样新增测试文件。提取片段、库、测试输入均保存 SHA-256；没有替换 router、准入、错误编码或后台任务实现。
5. 明确枚举并确认恰好两个测试，实际执行 **2 通过／0 失败／0 忽略**，测试耗时约 2.03 秒。这里的三个真实拒绝路由均实际运行；两种取消机制合计核验六个释放前的真实 429。
6. 同一定向程序通过 Clippy `-D warnings -D clippy::all`；新增测试文件 rustfmt 与 Git diff 空白检查通过。Clippy 仅生成 tmpfs 元数据，未生成新的工作盘原生构建产物。

该定向程序调用真实生产 router 的 oneshot 服务，不经过 TCP 或浏览器。验证使用关闭 core 的真实服务端库，因此默认 core 下的 discovered-disks／registrations 两条附加拒绝路径、Engine、完整 Cargo 单测目标及生产 WASM／Chrome **未执行**，仍需正式 CI。后台是用于确定性交错的阻塞测试 BackendProbe，不是实际磁盘引擎；本批没有操作真实磁盘或用户数据。

精确命令、依赖选择、资源记录、失败及成功日志、原样夹具、可独立重跑的定向二进制和补丁保存在独立树 `target/retained-management-slots-evidence/`。该证据目录不纳入源补丁。

## 统一一期候选的正式 core／TCP 补充

在 `c17e3ad90042ef83759c24da635449f24e14b35e` 上接入原 `6d3456662d0154526d71a22a5959aff7875ae8ff` 后，新增 [retained_management_slots_tcp.rs](../../test/server/unit/retained_management_slots_tcp.rs)。新文件作为原定向模块的 `core` 子模块接线，因此同一测试过滤器必须枚举四个测试：原有两个 router future 取消用例，以及两个真实 TCP 客户端断开／超时用例。

TCP 用例绑定内核分配的 loopback 端口，启动真实 `http_routing::serve` 与生产 router。真实 AuthStore 仅写入本用例的私有临时目录；不启动 Engine、不读取真实磁盘。请求通过 TCP 携带真实 Bearer，未使用注入 Authorization 的测试中间件。无凭据控制请求须返回 401 且后台调用数为 0；令牌只留在内存，不输出到测试证据。

两次 TCP 请求均先写完请求并确认后台实际进入 gate，然后直接丢弃拥有 socket 的读取 future，或让该 future 真实超时。这不同于取消只持有 JoinHandle 的客户端等待。两名后台尚未结束时，三个通用管理路由及 discovered-disks、registrations 共五条路由均须返回真实 429，逐一核对生产协议解码后的错误码、详情、retryable、非空且不同的 request ID；拒绝不得增加后台调用数。healthz 仍须成功。释放后有界等待许可回收，再读取准确的容量正文；服务端正常排空结束，失败路径同时释放 gate 并终止监听任务。

响应读取限制为 16 KiB、各等待上限 5 秒。选中路由均为已知缓冲响应，测试严格检查 HTTP/1.1、唯一 Content-Length 与完整正文，不用手造响应或忽略截断。客户端断开覆盖真实传输交错，但不宣称能从外部确定 Hyper 丢弃服务 future 的时刻；原有 oneshot 用例另行证明明确丢弃服务 future 后后台仍持有槽。两类证明需要一起保留。

共享 builder 的正式命令为：

```sh
cargo test --locked --offline -p storage-server --lib --features core backend_admission_tests::retained_management_slots:: -- --list
cargo test --locked --offline -p storage-server --lib --features core backend_admission_tests::retained_management_slots:: -- --test-threads=1 --nocapture
```

保留默认特性并显式要求 core。构建应使用统一候选的完整工作区与根 lock；稀疏源码副本缺失本地 Foundation 时不能当作可执行工作区。若分离 Cargo 编译和原生执行，须从本次 `--no-run --message-format=json` 输出选择唯一的 storage_server libtest artifact，检查该 artifact 的 features 包含 core，枚举并逐名核对四个测试后才运行。不能以零测试、旧无 core 二进制、源哈希一致或 metadata 检查代替该门禁。

本批只准备测试源码、模块接线及交接材料，不修改 `.github/workflows`、`.github/scripts/check.sh`、服务端限额或业务代码。受共享 builder 排队约束，新增 core／TCP 用例尚未编译或运行；上文已有的无 core 两用例执行证据仍保留其原范围，不能推广为本节通过。
