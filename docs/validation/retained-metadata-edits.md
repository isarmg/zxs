# 退出登记后的备注和文件分支名称

状态：源码及回归用例已编写，尚未编译、运行测试或执行 CI。必须与当前命名空间的三个精确操作准入一起整合；单独开放 gate 不构成交付。

## 操作范围

- `file.metadata`：新备注编辑要求稳定 `file_id` 仍出现在已准入的当前 branch 或 directory entries 中。查询使用当前 owned view 与准确 head，不把历史版本记录或历史身份禁止集合当作当前可见证明。
- `version.metadata`：显式版本备注仍按准确 `(file_id, version_id)` 定位历史记录。注销隐藏当前文件，不删除其历史备注入口，也不承诺该版本正文仍在线。
- `file.branch.rename`：目标必须是该文件的非迁移活动分支，不能是只读别名；在退出代际还须在该分支准确当前 head 的投影中保留该文件。既有名称冲突规则继续执行。

Disabled 保留当前条目，因此可以通过保留元数据成员记录备注或分支名称；它不恢复正文读写、登记写路由或原盘 I/O。Detached 的独占身份不接纳新的当前文件／分支编辑，但已发布的准确原请求继续返回永久收据。同名新文件由既有当前 workspace 流程取得新的 `file_id`，不会继承旧备注；重放旧收据也不重新执行旧编辑。

目录分支名称由当前命名空间实现单独接入：要求准确当前目录身份，并且只有除名称外全部快照数据相同才复用投影。不可变标签节点、merge、selection、历史导入等仍不在本批范围内，不能按“元数据”名称概括放行。

## 授权、发布与恢复

Engine 保留既有 trusted API 边界和非空 actor 检查。HTTP backend 在持有 Engine 锁后调用 `current_protected_identity`，再解析历史仓库及目标；新 mutation 与旧 receipt 都先经过当前身份验证，撤权不能借重试绕过。核心的 current-view 检查是目标存在约束，不是替代账户授权。

备注仅改变 `file_notes` 或 `version_notes`，分支标签仅改变 `Branch.name`，不创建正文版本、Commit、目录子树或文件元数据节点，不改变 head、generation、选中分支或路径。普通 publication 仍先执行 `CurrentNamespaces::advance`，持久发布成功后才同时安装 State 和下一 view：

- 备注没有 branch/directory 变更，直接共享原投影。
- 文件分支名称改变但 head 未变，沿用原 scope，不从不可变 Commit 重新导入已隐藏 entries。
- 冷启动的 `install_recovered_namespace_state` 与归档 `NamespaceReplay` 使用同一 advance；准确 D3 仍需由生命周期证明单独准入。

容器 `container.json` 只是 State 的派生缓存。`prepare_names_inner` 仍遍历有关文件的历史布局，但只有资源缺失且 `ValidatedConfig::excludes_io_disk` 明确排除该盘时，才省略该盘缓存投影。不是按 `NotFound`、普通离线或资源表缺失猜测注销。实际退出在完整 closure 之后才安装受限 Config；恢复先完成受认证的逻辑 I/O 排除。缺失普通保留盘的资源仍报错，持有资源的身份校验仍执行。被排除项不会进入 `PreparedNames` 的后续 apply 清单，不存在应用阶段按旧路径重新探盘的回退。

未减少成员、quorum、持久化阶段、同步或永久幂等收据。没有写入目标恢复文件，也不删除或复制目标物理字节。

## 成本与待执行用例

新当前文件备注最多遍历当前 branch/directory scopes 的 entries，成本随当前出现数增长，不遍历每条历史 Commit。显式版本备注改为版本表的准确键查找，并校验所属文件，避免原全版本扫描。分支标签检查只读该分支当前 entries；名称冲突扫描与既有发布、缓存成本保持原实现。没有新持久索引、缓存权威或额外 fsync 阶段，尚无性能实测。

`test/engine/unit/business/retained_metadata_tests.rs` 的 `retained_metadata_` 用例使用真实成员 fence、archive、joint decision 和 D3，定义以下回归：

- Offline/Detach 后可编辑保留文件、显式旧版本及当前文件分支；不可变内容和所有当前 namespace 不变。
- 退出目录移走、原路径改为普通文件后，编辑和重启仍不访问它；保存的原目录字节与占位文件不变。
- 隐藏身份拒绝新当前编辑，但旧成功收据继续幂等；显式历史版本备注及同名新身份彼此独立。
- Disabled 文件的元数据可写，正文仍不可读；普通未排除盘资源缺失不被吞掉；目标 I/O 尾部未关闭时不提前接纳新编辑。
- 启用 `fault-injection` 时覆盖 `after_finalize`、`after_container_names` 和首个 PUBLICATION 后恢复，保持投影、身份及目标无 I/O。

`test/server/unit/engine_backend/retained_metadata_tests.rs` 定义真实 D3 后当前文件和显式历史版本操作，以及未知身份和真实 session logout 后的新请求／旧 receipt 拒绝。只读源码复核不能代替执行结果；临时目录和故障注入也不等于独立物理盘或实际掉电验收。
