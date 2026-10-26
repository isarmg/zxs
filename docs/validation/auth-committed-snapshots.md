# 鉴权已提交快照验证（2026-10-26）

范围：[设计与线性化边界](../architecture/auth-committed-snapshots.md)。基线 `67c7d813`，
本轮仅改变服务端已提交凭据读取与相关组装、权威替换、回归夹具；不改变磁盘格式、
单管理员、SMB 非匿名要求、Origin/CSRF、密码及写任务上限，不包含真实部署。

## 已执行

使用仓库固定工具链，离线锁定依赖、独占 target、jobs=2、debug=0、incremental=0。

| 检查 | 结果 |
| --- | --- |
| `cargo test --locked --offline -p storage-server --lib -- --test-threads=2`，排除下列 6 项环境受限测试 | 460 通过、0 失败、4 既有 ignored |
| `cargo test --locked --offline -p storage-server --test http_engine --test login_admission -- --test-threads=2` | 55 + 3 通过，包含改密、CSRF、Origin、登录限流与恢复 |
| 最终扩展后 `cargo test --locked --offline -p storage-server --lib engine_backend::revocation_tests:: -- --test-threads=2` | 6 通过；覆盖 embedded/managed × 注销/改密 × 全 16 个排队入口 |
| 最后增加真实 rename 失败用例后 `cargo test --locked --offline -p storage-server --lib auth::snapshot_tests:: -- --test-threads=2` | 6 通过；原文件保留、候选回退、I/O 来源、临时文件清理与读者封锁 |
| `cargo clippy --locked --offline -p storage-server --lib --tests -- -D warnings` | 通过 |
| `cargo clippy --locked --offline -p storage-server --no-default-features --lib -- -D warnings` | 通过 |
| 改动 Rust 文件的 `rustfmt --edition 2024 --config skip_children=true` 与 `git diff --check` | 通过 |

新用例覆盖真实备份接口暂未完成时旧会话读可继续、新登录不可见；HTTP `/api/auth/me`
在注销持久化期间继续响应，完成后拒绝同 token；错误来源、确定失败回退与不确定失败封锁；
初始化/恢复沿用读通道；即使另一个读者短暂持有通道 Arc，写者销毁后仍不可继续准入。
没有新增生产测试 hook。Tree/Snapshot/Content 的 managed 读取持写 Mutex 时仍能完成，
但读与写同样在引擎准入点检查撤权。

首轮完整运行识别并修复了夹具直接替换权威、临时 MutexGuard 跨 router 构造造成的回归；
最终过滤运行及相关定向复跑通过。上述结果不是忽略产品失败所得。

## 环境受限／未执行

本地 AF_UNIX listener bind 返回 EPERM；保留原测试，不修改测试行为、不重试提升权限。
以下 6 项在最终 lib 运行中明确排除，仍须在真实 CI 执行：

- `service_lifecycle::tests::sockets::shutdown_closes_idle_smb_and_http_listener_while_http_request_is_stalled`
- `smb_rpc::tests::close_tests::failed_vfs_close_detaches_only_its_writer_and_keeps_both_sessions_usable`
- `smb_rpc::tests::close_tests::lost_close_reply_retains_original_published_operation_without_resubmitting`
- `smb_rpc::tests::cancellation_closes_blocked_rpc_sessions_and_removes_its_socket`
- `smb_rpc::tests::close_tests::poisoned_vfs_lock_detaches_without_publishing_or_discarding_unannounced_bytes`
- `smb_rpc::transport::startup_tests::bind_and_register_prepared_socket_preserves_permissions_and_identity_cleanup`

全 workspace `cargo fmt --all` 受 foundation 既有缺失测试模块接线限制；未把它记为通过，
也未保留无关上游源码的格式化改动。真实 Samba SDK、浏览器端及物理掉电测试未在本轮本地执行。
没有性能测量或吞吐提升结论；同步次数不变，每次写多一次最多 64 会话的有界快照 clone，
读只取共享引用，不等待凭据写序列的磁盘同步。
