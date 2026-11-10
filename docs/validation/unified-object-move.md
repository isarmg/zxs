# 统一搬运的关键流程测试

本文件描述测试源码的组织与剩余依赖，不是执行结果。2026-11-10 本轮只写测试，不运行项目、编译、格式器、测试或 CI；代码按独立分支汇总，集中整合后仍需获得运行许可。

## 按业务不变量分层

| 层次 | 复用入口 | 验证的关键事实 |
| --- | --- | --- |
| 请求与回执 | `test/contract/unit/lib/object_move.rs` | LookupPath 保留原输入，目标缺失必须显式 null；原 request、principal、digest 与事实一致；空盘完成后不把 None 改为今天的新仓库；旧 kind 不能伪装新 object.move。 |
| SMB 接纳与原操作续作 | `test/server/unit/smb_rpc/history_move_identity_tests.rs` | 真实 EngineBackend 与原临时盘夹具；同路径新文件不被旧重试搬走；源 entry/file 身份不被忽略；原 intent 使用同一 operation；NFD 源查询的原输入可在源消失后重放。 |
| 跨盘完整历史与一次发布 | `test/engine/integration/file_history_copy/move_execution.rs` | 保留历史正文、标签、分支和备注；源删除属于同一操作，不拆成 copy/delete；FILE、磁盘、SYSTEM 观察一致。复用现有测试，不另做模拟存储。 |
| 必要中断与冷恢复 | 同一 `move_execution.rs`、`move_recovery.rs` | 原 intent 后重试；决定后的已有注入边界重开同一操作；损坏正式正文拒绝激活，原证据和源内容保留。不得用新请求替代原操作。 |
| SMB 协议与空盘 | 既有 `empty_target_move_tests.rs`、`history_move_reply_tests.rs` | 实际 framed RPC、空盘首移、丢回执续作、覆盖模式不得改变；只有完全发布的原操作返回成功。脚本化回执测试仅证明适配器判断，不证明真实发布。 |

测试针对业务结果和不可丢失的身份／恢复约束，不追求逐行覆盖。优先在现有夹具上增加明确反例，不新增通用测试框架、生产 hook 或新的恢复协议。

## 当前源码依赖

- 契约测试依赖已写入独立候选的 ObjectMoveRequest、ObjectMoveFact 和 matches_receipt，尚未进入当前 main。
- SMB 完整源身份拒绝用例要求核心在原 receipt／intent 匹配中保留 source entry 与 file ID。旧实现只比较原路径及覆盖目标，适配器在已完成操作上跳过源核对，因此不能拿旧主线当本用例的正确实现。
- 当前 SMB 接纳测试直接调用真实 backend，未认证 Samba SDK、操作系统挂载、物理断电或浏览器行为。
- 目录和目录仓库的聚合单元、同仓 rename、覆盖目标完整身份，以及空盘 None 在一个核心操作中落盘的集成用例，待实际统一入口／桥接签名稳定后接现有 engine 夹具。不能以本文件或已写文件测试宣称这些流程完成。
- 现有早期测试仍有“目标只允许单个文件名”的断言，而新 PortablePath 契约允许父路径。业务入口完成后应按目标父目录真实存在与否更新断言，不保留旧限制来配合测试。

## 后续执行依据

以后获准运行时分别报告契约、真实 engine、SMB framed RPC 和真实 Samba 的结果，绑定准确组合提交。成功编译、某一层通过或静态阅读均不能代替其他层验收。首轮重点是一次业务变更、完整目标及保留历史、原操作幂等、授权/绑定改变、早期 intent 和决定后冷恢复。

