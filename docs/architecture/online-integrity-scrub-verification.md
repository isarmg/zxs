# 在线完整巡检：隔离早测记录

日期：2026-10-27 UTC。本文只记录隔离 source worktree 的实际执行，不代表组合矩阵、真实 Chrome、部署、设备压力或掉电门禁通过。

## 冻结输入

- 最终 source：`38897af5a962304e4d44bc52bc583c0949d126f3`，tree `94eac774d9a369dfe06aae4cef20afe3ddebe8ab`，工作树 clean。
- 核心最终修正：原提交 `17febe94e9614f583ec3276642294e6ef9e57d3f`（本地等价提交 `134cbedc6`）；包含 off-lock plan/batch、准确 frontier、真实尾后 1 byte probe 和最终 writer 状态核验。
- 服务源码：`06d134242`；HTTP UTC 投影：`eb3acf68a`；现有监测页消费者：`34b6d9e38`、`dffd698d9`；资源说明修正：`bca53f6fb`；启动 fixture 诊断修正：`38897af5a`。
- 服务单测在 `134cbedc6` / tree `eebc13f8c1b8029d3319e904761d4d21ce4fbf5e` 执行；随后只有启动 integration fixture 的诊断断言变化，生产源码未变。其余末轮检查在最终 source 执行。

环境使用项目固定本地 Rust toolchain，独立 `targets/online-service-check`，`CARGO_BUILD_JOBS=2`、dev/test debug=0、`CARGO_INCREMENTAL=0`。所有 Cargo 命令均 `--locked --offline`。最终 target 大小 1,200,292,790 字节，低于本轮 2 GB 上限；没有复用或清理其它工作者的 target。

## 实际结果

| 命令范围 | 结果 |
| --- | --- |
| `cargo check -p storage-server --lib --bins` | 通过 |
| `cargo check -p storage-server --no-default-features --lib` | 通过 |
| `cargo test -p storage-server --lib integrity_scrub -- --test-threads=2` | 14 通过，覆盖真实 HTTP 与原 `/admin/check`、单槽、周期、锁外预算、取消/排空、排队撤权、panic、重开及新鲜度 |
| `cargo test -p storage-server --lib management::shutdown_tests -- --test-threads=2` | 8 通过，含真实 recovery worker 的并发双 drain |
| `cargo test -p storage-server --lib service_lifecycle:: -- --test-threads=2 --skip service_lifecycle::tests::sockets` | 15 通过；明确未运行真实 socket 子模块 |
| `cargo test -p storage-server --lib shutdown::tests -- --test-threads=2` | 4 通过；1 个 child 入口由父测试实际派生执行 SIGINT/SIGTERM，普通 harness 标记 ignored |
| `cargo test -p storage-server --bin zxs -- --test-threads=2` | 7 通过 |
| `cargo test -p storage-server --test service_startup incoherent_scrub_host_policy -- --test-threads=2` | 修正 fixture 后 1 通过；不创建管理状态 |
| `cargo check -p web-app` | 通过 |
| `cargo test -p web-app --lib` | 271 通过，含新增 6 个巡检状态边界测试 |
| `cargo check -p web-app --target wasm32-unknown-unknown` | 通过 |
| `cargo check -p browser-smoke --bin browser-smoke` | 通过；只证明编译，不是浏览器执行 |
| `check-architecture.py`、`git diff --check` | 通过；修改 Rust 文件已 rustfmt |

首轮新增启动 fixture 断言错误地期望 Display 文案；daemon 既有外层实际使用 Debug，输出保留参数的具名 `InvalidScrubPolicy`。当轮 exit=1 且无目录创建，随后只把 fixture 改为断言真实具名诊断及两个无效参数，未改变生产错误处理。首轮失败日志保留为 `online-service-startup-policy-tests-1.log`，复测为 `online-service-startup-policy-tests-2.log`。其它本轮日志使用 `online-service-*.log`，保留在验证 workspace 根，不提交构建产物。

## 未覆盖及下一门禁

- 未在本隔离早测运行完整 server 组合矩阵或实际 socket 生命周期；没有为绕过本地限制更改测试、绑定策略或权限。
- 新增 Chrome fixture 已编译，会对照真实认证 HTTP 状态与监测面板；实际 Chrome、视口、无障碍和整套浏览器验收仍须中央/远端 CI 执行。
- 没有部署到真实池；没有物理磁盘故障、掉电、设备级 IOPS 公平性、前台延迟或大规模完整扫描时间测量。
- daemon 默认调度是结束后 24 小时、64 MiB/s 实际读取预算、从扫描开始计算 48 小时新鲜度，不是完成 SLA。此次验证不批准普通发布跳过旧依赖验证，也不代替增量发布责任的其它门禁。
