# 操作标识类型化与 SMB 领域接口

## 边界说明

基线为 `3fd13d3`。`OperationId` 私有持有 16 字节 UUID，`Copy`，直接生成；
`FromStr` 仅接受 `op_` + 32 位小写十六进制，包括全零和非 v4 值。
Display/serde 保持原字符串。类型不提供 Default、未校验构造、字符串 Deref
或旧字符串重载。

HTTP、已认证 SMB Session、救援 CLI，以及启动/恢复/导入反序列化负责解析；
核心变更、查询、计划、提交和恢复接口按值接收类型，删除重复 ID 校验。
actor、所有权、当前身份、fence 和资源/I/O 检查是独立约束，仍保留。

SMB transport 收到 `Request<String>`，会话转换为 `Request<OperationId>`；
解析失败保持连接、句柄与暂存状态，返回 `INVALID_REQUEST` 且无 operation_id。
普通业务 HTTP 变更保留格式错误 `INVALID_ID`，缺失/空串/非字符串 `INVALID_REQUEST`；
GET operation 的非空畸形标识仍映射 `OPERATION_NOT_FOUND`。
管理面 recovery-preview 在初次 HTTP DTO 边界解析一次，并将类型化请求传入
恢复流程；缺失/空串/非字符串为 `INVALID_REQUEST`，非空格式错误为 `INVALID_ID`。
空串从旧 `INVALID_ID` 改为 `INVALID_REQUEST` 是有意统一边界，而非保留偶然差异。
入口仍先检查 system-disk 当前登录身份，再解析 DTO；解析在引擎生命周期变更与
恢复 open 前完成。open 后动态 revalidate 保留，格式合法并不授予恢复权限。

`Backend::operation`、`Backend::commit_workspace` 返回 `OperationResult`。
Engine、Management、Authorized 三个实现显式准入/转发；SMB namespace 与
publication 直接使用它们，原内部 JSON commit action 删除。提交 DTO 位于
不依赖 core feature 的模块；HTTP/SMB 返回时共用原 wire 投影。原解锁 I/O、
重锁后当前身份与 workspace/owner/fence 校验时序保留。

仅 operations/operation_principals 两张 State map 改为 OperationId 键；
显式 Tracked::insert 标记 All。它们不参与历史 manifest，快照增量仅依赖
changes.is_empty，因此仍整字段重编码。分页边界哈希继续使用规范字符串，
不是 UUID 原始字节。

## 字节与行为证据

- 原真实 canonical_publication fixture 未改：State 字节、23 个 snapshot
  object、15 个 history object 及 root/hash 仍一致。
- 原固定 incremental snapshot root oracle 保留；新增 4096 个规范 ID 的
  字符串分桶对照、candidate 精确字节/domain hash、请求摘要对照。
- 两张操作 map 显式 insert 后的 All、字段重编码与历史独立性有回归覆盖。
- 原合成 manifest-sharing fixture 的 operation_id 原为非法字符串
  `operation`，现改为合法全零 ID；用独立 raw JSON 图变换重算，仅改变
  5 个 object 中的 commit page 及其引用 manifest 两个，真实冻结 fixture 不变。
- 原同 owner/input 重试、异 owner/body 拒绝、丢回复恢复、重开、降级恢复及
  导入用例继续执行。新增 typed Backend 两方法参与完成注销/改密后拒绝准入的
  barrier 竞态，并比较 state/workspace/journal 未变。
- raw JSON 和真实 FrameStream（UnixStream::pair）覆盖坏 ID 后原 read handle、
  FD 及同一认证 stream 仍可用，且不产生 workspace/spool 副作用。
- 救援 CLI 由参数解析器产生 OperationId；畸形参数在打开 pool 前失败。

## 复现命令

```sh
cargo test --locked --offline -p storage-contract -p storage-engine
cargo test --locked --offline -p storage-server -p smb-protocol
cargo check --locked --offline -p storage-server --no-default-features
cargo test --locked --offline -p storage-rescue -p storage-client -p authentication
cargo test --locked --offline -p web-app --lib
cargo test --locked --offline --release -p storage-contract \
  measure_operation_id_formatting_cost -- --ignored --nocapture
```

本地沿用已有 AF_UNIX bind 环境排除，不改变测试默认状态或 CI。
真实 Samba SDK 构建仍必须在 SDK CI 执行：VFS 的 no-default-features 构建由仓库
compile_error 明确拒绝，不能代替 SDK 验证。协议-only crate 独立验证。

## 行为验证（2026-10-26）

| 范围 | 通过 | 说明 |
| --- | ---: | --- |
| contract | 76 | 另有手动格式化微基准 |
| engine library | 551 | 6 个原有 ignored |
| engine integration / doc | 73 / 1 | 重开、降级、导入与证据校验 |
| SMB protocol | 34 | 原传输/能力及 raw/domain 编码 |
| server library / binary | 452 / 6 | 4 ignored，6 既有 AF_UNIX 排除 |
| HTTP / login / startup / telemetry | 55 / 3 / 6 / 1 | startup 另有 2 既有环境排除 |
| authentication | 17 | |
| client library / integration | 43 / 8 | 1 个原有 ignored |
| rescue unit / startup | 37 / 9 | 含 malformed ID 不打开 pool |
| web library | 255 | 正向领域 receipt fixture 改用合法规范 ID |

`perf-check` 与 `browser-smoke` all-targets 编译通过。

恢复 preview 边界追加验证：真实 HTTP 路由覆盖缺失、空串、null、数字、短串、
路径穿越、大小写与错误前缀；合法 ID 命中现有生命周期占用作为对照，畸形输入
在生命周期/open 前被拒绝且 system/disk 证据字节不变。注销后旧会话与无凭据
仍先返回 `UNAUTHORIZED`。新 HTTP 测试及原 open 后注销/改密重鉴权测试共 2 项
通过；server all-targets 严格 Clippy、no-default-features 检查通过。
这项新测试不计入上表原完整回归数字。证据日志为
`audit-r6-operation-id/preview-{boundary,clippy,no-core}.log`；协调窗口微测日志为
`audit-r6-operation-id/format-cost-quiet.log`（均位于工作区 worktree 同级目录）。


## 格式化成本

Linux x86_64，Rust 1.99.0（b940084d7），仓库 release profile，单个优化测试
二进制；固定规范 ID，每样本 1,000,000 次，7 轮交替次序，以 black_box 保留
结果。重测使用根任务协调的约 5 秒无 cargo 重载窗口，实际约 2 秒；
共享执行器未绑核，未声称全机独占。原测量日志保留，以下为协调窗口重测。
该微测仅覆盖内存复制/格式化及键哈希，不含完整快照、序列化、存储 I/O，
不能推导端到端吞吐。

| 轮次 | Display→String | 旧 String clone | Copy | 旧借用字符串 hash | typed 格式化+hash |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 53.648 | 8.879 | 0.589 | 86.192 | 130.445 |
| 1 | 50.191 | 8.471 | 0.556 | 81.802 | 126.354 |
| 2 | 50.150 | 8.351 | 0.556 | 81.239 | 132.053 |
| 3 | 50.588 | 9.165 | 0.567 | 85.350 | 132.151 |
| 4 | 52.153 | 9.107 | 0.597 | 82.272 | 136.149 |
| 5 | 50.734 | 8.296 | 0.556 | 80.235 | 163.301 |
| 6 | 50.347 | 8.985 | 0.632 | 81.372 | 129.592 |
| 中位 | 50.588 | 8.879 | 0.567 | 81.802 | 132.053 |

单位为 ns/次。维护旧分页边界需格式化规范字符串，测得中位键处理增加约
50.3 ns/键；这是明确保留的成本。OperationId 的 size_of 为 16 字节；String
头为 24 字节，规范文本长 35 字节。本次未测 allocator 元数据、容量或峰值 RSS，
不把表示尺寸差异宣称为实测整机内存收益。

contract/engine 与 server/SMB/rescue/client/web/perf-check/browser-smoke 的
all-targets 严格 Clippy（`-D warnings`）通过；所涉 crate 的格式检查与
`git diff --check` 通过。无新 crate、unsafe、生产测试 hook、operation receipt
截断或部署/权限变更。
