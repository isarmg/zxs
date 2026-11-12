# 原目录分支写控制的 Web 退役

## 实际范围与调用链

本片继目录初始化／暂存／提交入口退役之后，只删除原 directory-Git 的客户端写控制，不声明 server 或 engine 已删除。

- 当前目录详情的分支改名和设为默认按钮，分别经 `inline::Target::RenameDirectoryBranch`、`actions::select_default_branch` 调用 `mkdir` 的目录请求捕获。
- `staging::BranchReview` 将 directory-Git 状态、目录身份、工作树、头树、暂存代数与 namespace head 冻结为旧分支来源；`branch::Target` 的当前目录／历史目录变体调用对应 actions。
- 上述 actions 最终使用 `mkdir::storage::directory::DirectoryRequest`，生成 `directory.git.branch-create`、`directory.git.branch-create-history`、`directory.git.checkout`、`directory.branch.rename` 的原请求、摘要与回执匹配。
- 本片删除这条完整执行链以及专属恢复变体、checkout 默认视图完成／恢复接线、专属测试。没有把旧接口换名为 `.zxs` 新分支。

目录 checkout 同时影响默认读取与已启用 SMB 的目录暴露，所以该写控制一起删除。本 Web 没有另一个独立的 SMB 目录分支绑定请求。`smb_account` 改的是账户访问绑定，文件默认分支选择改的是当前文件暴露，均保留。

## 保留的边界与行为

- 文件容器当前版本／节点建分支、文件分支改名、标签、备注、节点图、文件默认暴露选择保留。
- 普通 mkdir、对象搬移和已有 ordinary-file-relocation 原请求的恢复编码／精确重试保留。共用恢复测试迁入 `namespace_recovery_tests.rs`，模块仍为 `files::mkdir::storage::tests`，覆盖账户与会话、请求关联、未知结果、预算、重复键、同操作冲突及失败清理后的重读围栏。
- 目录只读分支预览、不可变快照预览、所属目录／子目录导航、分页、响应解码／取消／代数关联和实体布局修复保留。`staging::Model::ready` 仍被布局修复消费；不能随分支来源删除。
- 普通 namespace 批次、目录删除／恢复、备份与放置仍有真实目录上下文消费者，不在本片顺带删除。`client::directory_branches` 是只读 GET，同样保留。
- 已有目录操作的历史展示文案及服务端错误码仍能显示历史结果；它们不产生写请求。

## 浏览器测试消费者

通知生命周期改为选择当前文件分支的真实默认暴露回执，仍检查八个导航目的地、关闭、五秒过期与迟到回执，共十一笔真实 PUT。选择当前分支保持文件正文／元数据，不为通知测试另外创建目录分支。命名测试改用文件建分支的 NFC 确认；三笔 mkdir 原请求仍检查浏览器原字节保留和清理，批次命名也继续覆盖。备份只读探针保留所有存活写控件禁用要求，并拒绝已退役的目录分支控件出现。请求观察器按 HTTP 方法区分目录写操作和只读 GET，文件默认选择不受影响。

## 浏览器保留记录

本片不兼容旧 `directory-branch-original-request`。旧记录不再能够解码、重试或被重新解释成 mkdir。共享 Store 仍保留不认识的原字节并返回加载错误，写围栏继续生效，不静默清除记录或假装完成。若存在未决旧分支操作，需先核对原操作结果及保留记录；本片不提供自动清理或转换。

## 新模式与验证缺口

当前仅继续文件容器相关能力；非容器文件、非容器目录及混合来源支持已停止，未完成候选保留而不启用。本片保留的 mkdir、对象搬移与目录导航属于现有文件容器逻辑命名空间，不代表继续开发普通文件系统仓库。新的 `.zxs` 分支功能尚未实现，容器快照来源仍应使用当前 SMB 暴露正文。本片不改盘上布局或核心持久契约，也不把旧目录分支预览称为新模式。

本地仅静读源码、编辑和生成差分／哈希清单；未编译、测试、rustfmt 或运行验证脚本，未操作浏览器、用户盘、Git 网络或发布。需由准确 main 提交的 GitHub 定向 CI 核验 native 行为、产品 native/WASM 编译及当前测试清单。真实浏览器验收、server/engine 旧目录写操作清理和新 `.zxs` 分支均仍待后续。
