# SMB Rename 的单一原生对象操作

## 背景与选择

默认共享已使用 `MoveCurrentObject`，命名共享却先尝试目录历史 export，再回到工作区内 Delete＋Rename。两条路径分别捕获对象、写恢复记录和解释发布结果；命名共享不能获得同一原生移动的替换及完整历史规则。

两种共享现在都由 `smb_rpc::object_move` 捕获当前 entry/file 身份，使用既有 `ObjectMoveRequest` 和 `MoveCurrentObject` 提交。显示路由只选取端点。`MoveContext` 借用原会话的主体、账户代数与 Shared，不建立新的授权状态或发布框架。工作区仍负责保存、mkdir 和结构删除，不再充当 SMB Rename 的第二个发布入口。

## 权威、原请求与恢复

- 命名共享固定 Hello 接受的 repository/main/binding generation。Engine 所有者锁内要求源与目标均等于这一绑定，拒绝跨库、换代和空盘目标。默认共享保留访问过的绑定索引，连接不能在禁用／重绑后采纳新代数。
- `MoveCurrentObject` 的源、目标磁盘身份与代数、原 entry/file 身份及当前主体继续在 Engine 提交处验证。完整历史复制、目录归属、D 节点、替换、发布与冷恢复由既有核心操作维护。
- 一份 `smb-object-move` intent 记录原请求；新记录直接编码一次，并在提交前读回比较准确原字节。恢复只严格解码已有字节，核对原账户、路径和绑定；不重新捕获已经消失的源或后来占位的目标。路径 owner 持续到收据核对、清理或失败返回之后。
- 只有与原命令完全匹配的 PUBLISHED、保护和健康收据才能清理记录。失败保留材料；已清理的 Namespace 操作号只能查询，不能重新提交。旧 server `smb-directory-history-export` 记录不转换、不删除。Engine 既有历史 export 的永久收据与恢复实现仍保留。

## 代价与行为边界

命名共享的成功响应改为同一个 `object.move` 回执。已显式提交文件跨目录 owner 时建立新完整历史身份；仅工作保存或暂存的文件继续保持原身份。类型不匹配的替换由核心返回 IsDirectory／NotDirectory，SMB 不复制第二套判定。

新的 native 捕获以提交时完整核心快照及捕获 entry/file 身份为准，不再借用工作区基线表达 Rename 前置条件。另一个操作在捕获后替换目标时拒绝原请求，不把同路径的新身份作为替换对象。工作区的基线、失败及不确定提交测试仍单独覆盖 mkdir／删除。

删除提交前重复的适配层 topology／repository 快照，Engine 的锁内最终验证不变；没有减少发布阶段、fsync 或独立副本。此设计不宣称吞吐改善，也不把 Rust 格式／静态审阅当作真实 Samba 或掉电验收。

## 验证入口

根测试覆盖命名共享丢失回包的原 intent 续作、原 binding 不扩权、未知格式保字节、捕获后目标身份替换及路径释放；现有公共 listener 测试同时执行命名与默认两种路由，保留未提交／已提交历史、替换、冻结旧版本、旧 reader、原收据与冷开断言。真实系统 Samba 和离线单 ELF 验收继续由准确提交的 CI 分别提供结论。
