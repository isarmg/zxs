# 普通目录复制的共享稿准入

候选：只改变新请求的只读业务准入，不改变持久字段、合法编码、永久回执或恢复器。

`DirectoryNamespaceBatch` 的 Copy 与 Separate 只复制已提交正文。现在在完整原 source/target
scope 和精确源 EntryId 均准入之后、构造正文或物理材料之前，检查实际内容来源子树中的 FileId。
Separate 使用明确 secondary occurrence 的正文来源，不用 retained 路径代选。任何选中身份具有
ManualDirty、ManualIncremental 或 Unborn 稿时，复用已有 `FileBranchConflict` 拒绝；零字节稿也不遗漏。

当前目录复制还需防止遗漏根本没有 Entry 的 Unborn 容器。仅当源目录属于当前实际选中 scope，
且当前命名空间包含同一个 EntryId/lookup occurrence 时，使用完整选中子树的当前目录 EntryId 集，
匹配 Unborn 已登记的直接 parent 身份。工作区 mount、历史同名目录、范围外目录和其他 FileId 不因
显示路径相似而被纳入。没有为未提交稿制造普通 Entry 或版本。

该检查在 `mutate` 的新计划 closure 内。已发表的相同操作／摘要／主体在进入 closure 前返回原回执；
摘要或主体冲突仍按原错误拒绝。已接受 Candidate 的恢复继续采用原材料，不调用这次 live 检查。
新请求被拒绝时不记录一个伪成功／空结果，也不创建或清理正文、清稿、建节点；提交或丢弃后可重新发起。
这不是对象整体搬迁的闭包实现；当前尚不支持的 whole-container move 仍保持原拒绝门。

新增 `business/shared_draft_copy_tests.rs` 源码覆盖真实新池及原生文件／工作稿：

- 选中文件有新稿时拒绝，比较原 State、disk inode/全部文件字节和未写入操作回执；
- 后来产生工作稿不阻止原成功 Copy 准确重试；冷开同样检查；
- 无关 FileId 仍可复制，原稿完整保留并可读取；
- 当前选中目录的真实嵌套 Unborn 阻止遗漏式复制，其他目录正常，冷开原稿不变。

测试源码已添加但未执行。未运行编译、测试、formatter 或 CI，也不声称 Separate、全部别名、
accepted/published 中断、真实 Samba 的端到端行为已验收。合并后的准确 head 需定向验证以上候选。
