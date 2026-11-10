# 当前 registry 下的备份配置证明

## 可交付范围

`Engine::configure_backups[_scoped]` 通过专用 planner 准入，普通 `require_operation` 继续拒绝 `backup.configure`。本功能重新保护所选稳定 scope 内仍为 Available 的全部历史正文，不生成新的正文版本，不移动历史 HEAD，不恢复已清除或已迁出正文。

正常发布保留原 fixed metadata quorum、全部成员发布屏障、实际正文复制和设备身份验证。正常 ACCEPT 补全、已发布结果冷恢复、永久幂等重试使用相同证明。代码必须与逐项 current-registry 冷前缀验证一起接线，不能仅添加操作名或只在修复之后验证。

本批不支持 current-registry 备份配置的显式降级恢复。该入口在验证原始候选后、搜索本地正文证明或观察损失前明确拒绝。原因是 PublishedDegraded 的认证结果覆盖与原生收据之间需要独立的证明和归档回放边界；不能把降级结果当成原始 TargetMet，也不能放宽原生收据比较。未形成原始完整副本发布时保留候选、日志和正文，等待后续完整恢复或独立实现的降级协议。

后续闭合必须同时修改普通恢复安装与控制归档回放，不能只删除拒绝条件：

1. 先按原始 hash-bound snapshot 验证原生 State 转换及精确 Candidate 副本集合，再证明 RecoveryProof、固定原成员与 publication barrier。结果只允许协议规定的降级字段覆盖，其他 actor、request、scope、路径、历史版本和策略字段必须保持原值。
2. `install_recovered_namespace_state` 目前会对已覆盖结果再次执行 `advance`；应消费从原始 State 得到的已验证 view，再安装经过独立证明的可见结果，不能把 Visible State 冒充原始收据。
3. `membership_transition/archive/namespace_replay.rs` 目前在回放前覆盖结果。归档需同时保留原始 State 与认证 outcome：本项先验证原始转换，后继项使用真实可见前态，snapshot 哈希链仍绑定原始根。`NamespaceReplay` 的冻结源结束校验也必须保持一致。
4. 验收至少覆盖真实 ACCEPT 后永久损失、显式降级、冷开幂等、后继业务写、再 Exit/归档/冷开，以及 outcome 与 source 任一字段伪造在副本 I/O 前拒绝。此项尚未实现或执行验证，不属于本批已交付能力。

## 纯计划和历史范围

- 来源为当前 main namespace 和该稳定目录的当前命名视图。不可变历史 Commit 和 DirectoryTree 继续保留，不能重新导入隐藏 occurrence。
- 来源绑定 registry commitment、mask、冻结 source snapshot/sequence/business generation、activation sequence、前态代数、main branch/head 和参与目录的 head。
- 稳定 scope 条件写入、策略 generation、独立子策略、移出文件的 retained policy 和固定历史副本链均保留。未来写入的各主盘链及全部可用历史版本的原主盘链都要求用户显式提供所需备份前缀。
- 只从已验证 registry 解析 Enabled 登记及其绑定代数。Disabled、Deregistered 或没有 Enabled 源持有者的范围失败关闭，不获得停止根的路径重开权限。
- GET 先验证 Enabled 主盘，再把不可用下游视为可用链终点，返回原策略代数/份数和可用上限；完整 requested route 不可满足时为空。PUT 不放宽，必须显式降低份数才能避开停止槽。
- 路径使用已有相对命名规则。保留现有产品语义：逻辑备份路径发生冲突时，备份列表返回 BACKUP_PATH_CONFLICT；不同历史/稳定文件容器仍物理隔离。本批不悄悄改变配置端冲突语义。

计划的精确写集仅为 scoped/retained policies、文件策略绑定、用户备份路径、所选可用历史版本 placement、一个新操作与主体、一个事件及普通发布代数。穷尽 State 解构保持其余字段不变，包括 FileVersion、Commit、DirectoryTree、物理名称/位置、旧永久收据、认证及拓扑。独立修改旧备份事件或主体同样拒绝。

## 实际副本与来源

State 证明不能约束独立的 Prepared 或 Candidate.versions。专用 planner 和 publication 的首次正文探测前都检查实际 Prepared 的数量、顺序、完整 FileVersion 和 Placement；适用性从真实 State 新收据确定，不能用另一个外传 result 绕过。

当前备份正文源由 `valid_backup_replica` 创建。它与原 `valid_replica` 共用现有本地发布证明、完整正文校验及 held I/O owner 路径，但返回私有字段的不透明来源，精确保存版本、原路径和 owner。仅有相同盘 seal、另一路径、外部文件或相同字节都不能伪造该来源。对来源拼接路径会丢弃这个证明，不增加磁盘读取。其后的副本验证、写入前完整校验、独立副本准备、decision 边界 seal、ACCEPT、publication 和本地 proof 链全部保留。

统一冷入口先用真实前态/current view 验证原生 State 转换，随后精确比对 Candidate 的完整版本与 placement 集合，才允许搜索 local publication proof 或修复。只有成功证明的真实备份转换可以重用旧版本；其他已准入正文操作仍受其完整 fresh-version 增量及本次实际可写 namespace 引用约束。去掉 kind、标记或整个业务 delta 都不会获得任意旧正文探测权限。控制归档的 `NamespaceReplay` 同样在原生 State 转换之后验证精确 backup Candidate 集合，不因来源是归档而省略证明，不打开历史磁盘。

## 验证源码与成本

定向回归在 `test/engine/unit/engine/current_backup_configuration_tests.rs`。保留原真实 D3 Offline/Deregister、两份历史正文、冷幂等、隐藏目录视图、子策略与移出文件、scope/generation、所有 State 字段和冷解码攻击测试。新增：

- Prepared 缺失、额外、重排、版本/placement 改写、外部源、同 owner 换路径、跨版本来源及外传 result 改名，均在 publication I/O 前拒绝。
- ACCEPT-only 与 Published 的真实重哈希 Candidate/snapshot/history/固定成员日志矩阵，覆盖版本缺失、额外、重复、错误 placement、来源证明改写、改名去标记及删除整个业务 delta。
- 活跃 data-only 版本目录的 OPEN/ACCESS 观察，先用真实 commit-proof 读取正校准。候选快照缺一份副本时，非法候选仍应在修复、名称投影和正文证明读取前拒绝；停止根搬走并在原址放普通文件，原始保留字节不变。
- Purged/Migrated 历史版本不参与补副本；旧备份事件不能独立改写或重复追加。
- 明确拒绝 current backup 降级预览，真实正文/证明目录无 OPEN/ACCESS，日志和原候选保持；Disabled 下游的 GET 展示及显式降份数不重开停止根。
- 正常 backup 之后再次移除真实 metadata voter，归档重放该 backup；两个停止根都搬走且原址放普通文件，冷开后保持两份历史正文与永久幂等结果。

2026-11-05 在 Linux 临时目录、Rust 1.99.0 下，对组合接线后的工作树执行：

- `cargo test --locked --offline -p storage-engine --lib --no-run`：通过，生成实际 libtest；首轮发现的策略 ID 生成闭包错误类型和测试常量导入问题已修复后重编。
- 同一 libtest 的 `engine::disk_registry::current::backup_configuration::tests --test-threads=2`：15/15 通过，198.73 秒。包含 ACCEPT-only／Published 重哈希矩阵和带正校准的 inotify OPEN／ACCESS 断言，不是只做 State 单元比较。
- 同一 libtest 的 `protection::backup_paths::tests --test-threads=2`：36/36 通过，10.28 秒。
- 同一 libtest 的 `engine::reads::tests --test-threads=2`：40/40 通过，1.17 秒；覆盖来源替换、坏副本回退、持有身份、撤销及读取资源释放。
- `python3 .github/scripts/check-rust-diagnostics.py -- cargo clippy --locked --offline -p storage-engine --all-targets --message-format=json -- -D warnings`：通过，36.70 秒，零 Rust warning/error；包含真实库、单元与集成测试目标。
- 本批 Rust 文件的 `rustfmt --edition 2024 --config skip_children=true --check`、`git diff --check`、第一方架构检查：通过。架构检查覆盖 20 个 owner、31 条声明依赖。

同时补齐跨模块降级拒绝入口的最小可见性，并将停止下游的 GET／显式降份数测试扩展到 PlannedOffline 和 Deregister 两种真实 D3，验证降份数后的冷开永久幂等。上述 91 项在最终生产实现（包含不透明来源的 Arc 共享及组合前缀／标签证明）上复跑通过。完整仓库运行、真实设备掉电、实际 SMB 客户端及物理独立副本验收不属于这些定向通过证据；本批未以临时目录替代这些验证。

纯计划需要遍历 scope、相关目录、策略和版本，符合全部可用历史版本重新保护的原有语义。来源证明不增加文件 I/O，每个版本分配一份共享的不可变来源记录，持有既有 FileVersion 与 held owner/path；克隆这个来源变体只增加 Arc 引用，不扩大 ReadSource 的最大变体或深复制版本描述。不改变锁次序、decision barrier 或 fsync 阶段。冷前缀验证会增加原始快照解码/证明工作；不宣称零成本、吞吐提升或物理断电验收。
