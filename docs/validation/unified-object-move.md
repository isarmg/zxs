# 统一搬运的关键流程测试

本文件描述测试源码的组织与剩余依赖，不是执行结果。2026-11-10 本轮只写测试，不运行项目、编译、格式器、测试或 CI；代码按独立分支汇总，集中整合后仍需获得运行许可。

## 按业务不变量分层

| 层次 | 复用入口 | 验证的关键事实 |
| --- | --- | --- |
| 请求与回执 | `test/contract/unit/lib/object_move.rs` | LookupPath 保留原输入，目标缺失必须显式 null；原 request、principal、digest 与事实一致；空盘完成后不把 None 改为今天的新仓库；旧 kind 不能伪装新 object.move。 |
| SMB 接纳与原操作续作 | `test/server/unit/smb_rpc/history_move_identity_tests.rs` | 真实 EngineBackend 与原临时盘夹具；同路径新文件不被旧重试搬走；源 entry/file 身份不被忽略；原 intent 使用同一 operation；NFD 源查询的原输入可在源消失后重放。 |
| 跨盘完整历史与一次发布 | `test/engine/integration/file_history_copy/move_execution.rs` | 保留历史正文、标签、分支和备注；源删除属于同一操作，不拆成 copy/delete；FILE、磁盘、SYSTEM 观察一致。复用现有测试，不另做模拟存储。 |
| 新对象共用完成屏障 | `test/server/unit/smb_rpc/history_move_completion_tests.rs` | 一条流程参数化首 PREPARE 前与 SYSTEM 后两个边界，并分别复用原绑定部分缺失、成员部分退休；离线保留模式不得用旧 State 重建已安装候选 SYSTEM；正常冷开完成原候选，三范围可读、全体原绑定字节退休、单事件及幂等回执。不按对象种类复制矩阵。 |
| SYSTEM 丢失与完成依据 | 同一 `history_move_completion_tests.rs` | 复用真实临时盘，删除 SYSTEM 后从原 metadata 成员恢复同一 State、完整正文和回执；全部完成归档丢失必须拒绝且不补造，放回准确原字节后才允许恢复。 |
| 必要中断与冷恢复 | 同一 `move_execution.rs`、`move_recovery.rs` | 原 intent 后重试；决定后的已有注入边界重开同一操作；损坏正式正文拒绝激活，原证据和源内容保留。不得用新请求替代原操作。 |
| SMB 协议与空盘 | `object_move_tests.rs`、既有 `empty_target_move_tests.rs`、`history_move_reply_tests.rs` | 实际 framed RPC 同盘目录改名、空盘首移、丢回执重连及原操作续作，覆盖模式不得改变；只有完全发布的原操作返回成功。脚本化回执测试仅证明适配器判断，不证明真实发布。 |

测试针对业务结果和不可丢失的身份／恢复约束，不追求逐行覆盖。优先在现有夹具上增加明确反例，不新增通用测试框架、生产 hook 或新的恢复协议。

## 当前源码依赖

- 契约测试依赖已写入独立候选的 ObjectMoveRequest、ObjectMoveFact 和 matches_receipt，尚未进入当前 main。
- SMB 完整源身份拒绝用例要求核心在原 receipt／intent 匹配中保留 source entry 与 file ID。旧实现只比较原路径及覆盖目标，适配器在已完成操作上跳过源核对，因此不能拿旧主线当本用例的正确实现。
- 当前 SMB 接纳测试直接调用真实 backend，未认证 Samba SDK、操作系统挂载、物理断电或浏览器行为。
- 既有 Engine 成功路径要求新 object.move 和公共 move-completion 归档；原 file intent／决定后恢复先用同一 capture/project/save 夹具准备真实历史材料，继续要求原 file.history.move。成员退出进行中保持写围栏，明确激活后的合法 Current 允许新搬运；这两阶段在同一用例复用，不把终态当作过渡态拒绝。
- 泛化 SMB 接纳夹具已转发真实 MoveCurrentObject，原文件桥接只负责已有请求身份与旧 intent 续作；完成屏障直接走新对象入口。空盘 framed RPC 保留原 target_repository_id／target_binding 为 None，并要求同一操作完成根与搬运。
- 泛化 command 必须携带原源 slot/disk/generation 和原目标 disk/generation；真实 backend 回执重放仍核两端磁盘。object 回执须显式提供 version_mapping，空 map 对应 NotApplicable、非空对应 TargetMet，缺失不默认为空。
- 已发表回执的两端 SMB 绑定检查直接覆盖 `MoveCurrentObject`：撤销后拒绝，重新启用同一仓库也不能让旧代数的原捕获句柄恢复权限；拒绝不改原回执或 State。
- 目录／目录仓库正常流程由公共 Engine 测试覆盖，不在协议层复制成员和故障矩阵。协议层只补目录显式 None 身份、same-entry 不产生 displaced file，以及新 object spool 不接受旧 file kind。
- PortablePath 父路径与允许覆盖但目标缺失均由接纳测试转交核心；脚本化成功只证明输入映射，不证明缺失父目录能发布。目标父目录存在性和完整成员保留留在业务测试。

## 后续执行依据

以后获准运行时分别报告契约、真实 engine、SMB framed RPC 和真实 Samba 的结果，绑定准确组合提交。成功编译、某一层通过或静态阅读均不能代替其他层验收。首轮重点是一次业务变更、完整目标及保留历史、原操作幂等、授权/绑定改变、早期 intent 和决定后冷恢复。

