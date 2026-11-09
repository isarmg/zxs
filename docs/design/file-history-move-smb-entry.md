# 默认 SMB 共享的原生跨盘 move 入口

## 状态与依赖

服务端协同候选 `265a437e` 已与同父 `bcf065da` 上的
[`FileHistoryMovePathRequest` 与 `Engine::move_current_file` 共同执行源码](file-history-move-execution.md)合流，
没有为缺失的核心证明添加替代实现。合流源码待唯一合并槽，尚不能声明可用或已验收。
本片没有本地编译、构建或运行测试、触发 CI，也没有操作真实盘或 Samba 服务。

## 唯一语义与范围

默认共享的跨主盘 `Rename` 只路由为一笔 `file.history.move`。同库 rename 的
原有入口不变。跨盘请求不使用 workspace 复制、普通 copy、软删除或硬删除拼装，
也不制造第二笔 `file.delete` 收据。

两端必须是现有磁盘根仓库，不能是命名／初始化目录库。源可以是盘根未初始化
子目录内的单文件，但核心必须拒绝多重当前 occurrence。目标是另一个主盘根中的
单个可移植名称。`replace`、目录目的路径、现存目标条目均拒绝。目标盘没有根库时
只返回原只读路径解析错误，不隐式创建根库；目标库必须为空、成员和原资源固定等
进一步限制由同一核心准入执行，不由 SMB 快照放宽。

## 捕获与授权

1. 默认共享保留整个请求期间的账户选择读 guard，先核准确账户世代。
2. 两个仓库分别沿连接已有规则 pin `repository / main branch / SMB generation`。
   重新启用或换绑定不能让旧连接重新接纳新世代。两份 bound identity 分别授权。
3. 一次持有共享 writer registry 锁，检查并同时登记两端 `(branch, path key)`。
   大小写／Unicode key 和祖先／后代重叠规则与普通 SMB 写者一致；冲突时不留下
   半边 reservation。guard 跨源条目捕获、意图落盘、核心调用和结果分类。
4. 捕获源 `entry_id / file_id`。EngineBackend 在同一个 engine owner 锁内重新核
   当前管理员 admission、两份 SMB binding、两端根库／主分支以及源身份。
   已存在 operation 先交核心按原永久收据对账，不把今日源消失当作原操作失败。
5. 核心四字符串请求只含准确的 `source_repository_id / source_path /
   target_repository_id / target_name`，并传原 principal 和 operation。固定成员、
   全历史、唯一 Candidate、原 head／file／clock／slot 的恢复都留在核心，不在
   RPC 层重新构造或绕过。

## 持久意图和成功门

沿用 namespace operation reservation 与预算。第一次核心调用前，用不覆盖安装
写入并同步 `<operation>.namespace.json` 及其父目录；格式为
`smb-file-history-move`。记录保留四字段原请求、两端绑定、源条目／文件身份、
账户世代、principal、原 operation 和 `unconfirmed`，没有密码或令牌。

文件或目录同步失败都不调用核心，保留已有材料。核心错误、错误 operation／kind，
以及任何非完整成功状态均保留记录。唯一可返回 SMB 成功的结果是同一个 operation
的 `file.history.move`，且同时为 `Published / TargetMet / Healthy`。Prepared、
Decided、Acked、PublishedDegraded、未知和失败状态不会因 `Ok(OperationResult)`
而返回成功。只有通过此门并成功编码后才尽力清理 namespace 记录，清理 I/O 失败
记录诊断，不撤销已经成立的核心 publication。

错误一直携带原 operation，禁止自动重试。现有 namespace admission 会拒绝已有
收据或原 spool 记录，不能换路径重放同 ID。spool 是恢复材料，不是决定、授权、
客户端新请求或重放许可；原决定的继续恢复仍须走核心原请求与收据闭包。

RPC 结束释放临时路径 guard，不清核心 choice。未知之后的普通写入由核心持久围栏
拒绝，两端读可见性由同一个旧／新 State 共同切换，不能通过物理数据库交换自行
授权。SMB 内存 reservation 不是崩溃恢复围栏，也不声称锁住全部 HTTP／FD 别名。

## 成本和验证边界

两端最多两个 writer key，复用原有按现存写者扫描的重叠检查，锁不跨 I/O。路径
查询和两端授权各自有界，核心前另有同锁身份核对。意图沿用 namespace 的两次
持久同步及 64 KiB 预算开销；未知材料继续计费，不按成功上传或 Flush 释放。

新增 16 个 Rust 测试函数仅为源码：完整成功门、各非成功状态、错误收据、目标
单分量、双绑定传参、未知／失败意图保全、不自动重放、同步故障、非覆盖、两端
reservation、账户世代、绑定世代和 owner 锁内源身份核对。脚本化成功回复只测试
适配器成功门，不证明核心确实发布。现有空目标盘测试保留“不创建目标根库”，
错误期望改为实际 `DirectoryNotFound`；非空目标测试继续要求核心拒绝。

只做修改文件源码格式整理；上述测试没有运行，尚无核心组合、编译、真实 SMB、
断电或冷开证明。目录／多文件、非空目标库、覆盖、成员变更和跨命名库不在范围内。
