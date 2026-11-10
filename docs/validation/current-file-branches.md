# 退出后的当前文件分支准入

本切片接续[当前磁盘命名空间](current-disk-namespaces.md)，覆盖真实 Exit publication 后的文件分支创建、选择、默认分支与分支合并。它不增加设备操作、目标盘恢复备份或旧格式兼容。

## 来源与写入边界

- 创建分支仍是 pending 元数据操作，不复制正文。文件 occurrence、准确 entry/file identity、路径及全部父目录来自拥有仓库的当前投影。历史版本必须属于同一文件／历史域；已无登记副本的版本不能作为新当前分支来源。
- 新分支的准入证明绑定实际当前 owner HEAD、fork、父提交、单一文件 occurrence、祖先、正文身份／大小和 pending 引用。普通发布及恢复根据同一前后 State 重算，不能靠全局已知 identity 集合恢复旧条目。
- 选择和默认分支从 `NamespaceSnapshot` 取得当前文件来源，目标路径也在当前 owner snapshot 中匹配准确 file identity。发布沿用当前 entries 并移除隐藏 path context，不能借同名新文件替换旧身份。旧 workspace 草稿发布仍走原有独立写入验证，不把刚写出的 draft HEAD 冒充旧 snapshot。
- pending 选择只保留继承意图；materialized 选择和所有合并必须通过真实副本校验。读取使用现有资源准入，退出盘的历史 route 不会重新打开目标资源，仍可从真实幸存副本读取。
- owner、来源文件分支及实际改变的目录分支须保有当前写权限。Disabled 仍保留登记和条目，但元数据选择不能给它授予正文写入能力，也不会自动把 backup 提升为 primary。

## 合并闭包

合并先验证每个当前输入的准确节点与投影正文，再独立处理当前 named directory scopes。每个文件路径按自己的 context 匹配参与分支，未参与分支不被全局 file ID 覆盖；当前未显示的目录分支也参与。

引用的 child tree 必须是该准确 child branch 的当前已准入 HEAD，其投影 entries 和上下文必须与父引用一致。所选非 HEAD 历史树、历史 child 引用、上下文不闭合的组合明确返回 `RecoveryRequired`，不会调用旧递归发布器导入任意 frozen tree。即使目录 entries 表面上安全，缺少这个闭包证明也不放行。

已验证的当前目录按深度从子到父发布新的不可变树。新文件 HEAD、owner HEAD 和目录子树都从各自投影 entries 构造，只改变参与文件的正文／节点引用。旧 Commit、DirectoryTree、版本、节点与合并来源保持不变；旧版本的显式历史读取仍是只读历史语义。成功操作的永久 receipt 在当前路径／节点检查之前重放，不因后续合并、退出或重开而失效。

## 源码测试与验证状态

新增 `test/engine/unit/engine/current_file_branch_tests.rs`，通过真实临时池、控制归档、joint decision 和 D3 activation 驱动：

1. 当前文件从仍有幸存副本的历史版本创建 pending 分支，materialized 选择及默认切换，选历史正文合并，未选择目录同步、重开及永久 receipt。
2. 隐藏文件同名重建后得到全新 entry/file identity，旧身份的创建、选择及默认请求失败，显式历史仍保留旧条目。
3. Disabled owner 的创建、选择、默认、合并及 workspace 写入口拒绝；原有成功 receipt 仍可重放。
4. 没有幸存正文的历史版本不能创建当前分支或成为合并结果，失败不发布 State。
5. 冻结父目录持有非当前 child HEAD 时拒绝合并，原状态不变。

测试同时检查退出资源关闭证明、实际 target 文件字节不变、旧 Commit／DirectoryTree／version 不变；这些断言不是物理设备和掉电验收。

本切片只完成源码审阅、官方 Rust 1.99.0 `rustfmt --edition 2024 --config skip_children=true` 和 `git diff --check`。**未执行编译、Cargo check/clippy、单元或集成测试、模型及 CI；不宣称测试通过。** 整合后的负责人应运行上述回归、已有文件历史合并、文件 pending、目录组合、当前 namespace 和真实退出／恢复测试，并记录最终提交的结果。

## 成本

创建和选择仍复制本次变更的 namespace entries，并使用既有不可变元数据发布；不创建 FULL。新分支证明只检查准确 owner 和目标快照，不扫描全部历史正文。

合并扫描／复制该仓库的当前 named-directory entries 与上下文，并校验实际引用的 child 闭包；目录查找使用键索引和连续后代区间，不逐条扫描全历史树。额外内存与本次当前投影总大小及引用数量相关，每个改变的目录新增一份 commit/tree。旧 ancestry、永久 receipt 和持久化事务流程未改，也未增加独立锁或 fsync 阶段。

真实副本验证仍按被采用正文的字节量执行一次；未测量时间、RSS、I/O 或同步次数，不以静态复杂度宣称吞吐提升。
