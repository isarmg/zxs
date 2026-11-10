# 外来历史写入首个纵切（实验分支）

首个实现只保存准确目标数据盘上的持久执行意图；它不是复制启动、目标预留、磁盘准入、命名空间发布或导入成功。现有 plans/execute 拒绝保持。架构取舍见 [ADR 0016](../adr/0016-foreign-import-intents.md)。

## 明确的格式与范围

第一阶段仅使用当前生产 managed-directory / membership-generation 来源和当前 Engine 已登记的一级目标。来源身份是当前格式的 pool/disk/slot/binding_generation 四元组，并保留完整 publication/checkpoint、原始证明与实际本机目录身份。它与独立逐盘原型的 origin+incarnation+role 格式是两条显式格式边界；不为旧四元组补默认 incarnation，不把实验性逐盘 receipt 接入生产准入。

一个物理来源可以包含多个原一级盘的多条完整历史。每条验证历史必须恰好映射到一个唯一子目录；元数据、原来源或 publication 不完整必须拒绝。原先缺失的正文保持暂不可用，已记录 purged/migrated 的状态不改写，不以缺失正文为由删除历史。第一版不做角色转换、原来源写入/清理或接管。

## 本轮阶段

1. 完整来源检查和 raw inventory 在 Engine 锁外完成，持有本次已准入 backend，不用健康缓存的第二次准入替代当前 auth/stop/fence。
2. 当前用户、独立执行 operation、完整来源证明、目标 registration/物理身份、namespace checkpoint、全组映射及保留缺失正文策略共同进入准确摘要。保存意图前重新验证当前身份、停止状态、成员写围栏和目标基线。
3. 仅从已准入 Engine 的准确 DiskRoot 写目标 metadata/foreign-import-intents。每个操作使用独立目录；创建后失败留下的无完整 intent 目录必须报 RecoveryRequired，不能作为不存在、自动清空或用新输入接管。完整相同 intent 幂等，不同 intent 保留并拒绝。
4. 保存前检查单记录上限、目标盘所有保留记录/部分失败残留的总数量和字节，以及真实余量与 metadata reserve。状态查询不读取源路径，不通过管理 JSON 或 SQLite 推断成功。

初期固定上限为每个 intent 2 MiB、每盘 128 个保留操作及 256 MiB 总保留字节；达到上限拒绝新增，不删除或过期旧记录。无 intent 的部分目录也占用操作数。该约束后续可以经审查改成显式配置，但不得裁剪永久幂等证据。

## 尚未开放的阶段

独立完整副本复制、数据盘 reservation 对普通写和成员变更的协调、原 ID 全域冲突证明、多 history 原子 namespace 安装、故障恢复和永久 ImportPublicationReceipt 尚需实现及真实 I/O 故障测试。本轮不得把 IntentSaved 或离线 CapsuleReceipt / DiskPublicationReceipt 显示为 Completed。后续 writer 必须重新获得当前权限和目标写能力，不能由反序列化 intent 直接生成。

## 已接入的管理调用边界

新增 POST `/api/admin/storage/foreign/execution-intents/preview`、`save`、`status`，全部沿用当前会话、CSRF 与存储生命周期门。preview 选择已有完整只读 plan 和独立意图 ID，返回明确的 `save_execution_intent` 效果。save 必须携带原样完整 preview 以及 `confirm_save_intent=true`；摘要绑定系统审核 ID/摘要及当前主体，不接受遗漏历史、替换目标或自行重写摘要。返回仅有 `stage=intent_saved`、`execution=unavailable`，没有 Completed/Published 变体。

新请求在完整源扫描前只准入一次 backend；慢扫描后持同一个 Engine，在实际保存锁内再次核当前会话、service stop、成员 fence、目标 namespace 与物理根。原样保存重试优先核同目标、原 ID、原摘要的已存记录，同 FD 重新建立耐久屏障，不依赖源仍在线或 namespace 基线仍空。status 只读已选择目标的真实记录，不重新探测客户端路径。

意图的唯一性范围明确是 `(完整目标 registration identity, intent operation_id)`；它不预留全池业务 operation ID，也不排斥另一目标盘自己的同名记录。当前已存在业务 operation 的 ID 在保存/同步时拒绝；以后出现同 ID 业务记录，不会覆盖意图，意图仍能只读查询，但同步拒绝。未来 importer 必须另建立真实全池事务与 ID 归属，不能把该局部记录当业务回执、全池锁或 namespace reservation。

首次保存前通过两个已持有根 FD 检查实际包含关系。完整祖先遍历有 256 层上限；同文件系统的不同 mount view 暂明确拒绝，避免子目录 bind mount 隐藏原祖先。MNT_ID 只用于本次实时检查，不进入持久身份，未假设跨重挂稳定。没有执行真实 mount 权限验收。

## 验证边界

引擎验证覆盖完整多来源、缺正文、冷读、原样重放、目录/文件替换、不完整记录、严格必填 null、总保留预算及真实 metadata reserve；跨目标 ID 与后续业务冲突单独验证。管理会话/停止/成员围栏和 HTTP 请求门分别测试，不能互相替代。真实 bind mount 与物理掉电验收需要单独环境，不用模型或编译通过代替。实际复制、目标 namespace 发布及其跨盘决定恢复仍未实现。
