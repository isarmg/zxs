# ZXS 架构图

[顶层设计](../ARCHITECTURE.md) · [当前范围与验证](status.md) · [术语](glossary.md)

## 1. 模块与数据

```mermaid
flowchart LR
    Browser["Rust/WASM 管理页"] -->|HTTP 管理和元数据| Web["management：Web 会话及表单"]
    Client["文件客户端"] -->|签名 SMB 3.1.1| Smb["smb：协议、会话和句柄"]
    Web --> Core["system：池登记、当前授权和操作结果"]
    Smb --> Core
    Smb -->|只读 SMB 校验材料| Vault["实例私有 vault"]
    Core -->|一个 SQLite 库| S["S：SYSTEM.sqlite3 和回滚日志"]
    Core --> File["ordinary：FILE 材料和直接区间索引"]
    File -->|使用持有的原目录及文件| Fs["fs：本机路径、挂载和同步"]
    Fs --> F["F：普通目录及 FILE 容器"]
    Fs --> R["R：独立回收文件系统"]
    Helper["独立 Helper：离线首设和维护"] -. 同一数据契约 .-> S
```

Web 通过 `system` 读取获准的名称、共享和历史；手动提交时核对普通盘材料，不把正文送到浏览器。宿主负责挂载 S、F、R。Helper 停机写入采用同一格式，不是 ZXS 的运行依赖。

源码：[Web](../src/management.rs) · [浏览器](../src/browser/) · [SMB](../src/smb/mod.rs) · [SQLite 入口](../src/system/reader.rs) · [FILE 工作稿](../src/ordinary/flat/) · [实际文件系统](../src/fs.rs)

## 2. root 前台怎样放行 Web 和 SMB

```mermaid
sequenceDiagram
    participant Root as root 前台
    participant Check as 降权预检
    participant SMB as 降权 SMB
    participant Web as 降权 Web
    Note over Root: 核准 ELF 和站点，持 S 共享锁和站点独占锁
    Root->>Check: 同一 ELF，只读 S；继承 fd3 控制套接字和 fd4 父 pidfd
    Root->>Check: 经 fd3 交付封存的公开启动资料
    Check-->>Root: 已核对凭据代和卷关系，仅回公开结果；预检退出
    Note over Root: 前台确认后准备角色挂载视图、监听套接字和同一放行 memfd
    Root->>SMB: 执行同一 ELF；继承 fd3 和 fd4，投影 S、所选 F 和可核的 R
    Root->>SMB: SCM_RIGHTS 交付封存公开资料、监听、只读 vault、放行读端
    SMB-->>Root: PREPARED 后关闭回执流
    Root->>Web: 执行同一 ELF；继承 fd3 和 fd4，投影 S 和只读 F
    Root->>Web: SCM_RIGHTS 交付封存公开资料、HTTP 监听、放行读端
    Web-->>Root: PREPARED 后关闭回执流
    Root->>Root: 核对两份回执与存活状态，在同一放行 memfd 加 F_SEAL_SEAL
    Note over SMB,Web: 各自验证同一封印后才接收连接
    Root->>SMB: 停机时先发 TERM
    Root->>Web: 等待回收前也发 TERM
    Note over Root,Web: 排空、回收两个原进程后释放站点及原 S 持有
```

`fd4` 指向原前台进程，不是文件数据。投影是子进程看见的挂载树，不经启动消息转交数据盘 FD。预检的 S 强制只读；Web 的 F 强制只读，S 保留原挂载模式，允许在可写 S 上提交获准的 SQL 变更。Web 没有 R、vault 或 SMB 监听。前台不打开 SQLite；两个业务进程各自重新核对原 S、安装描述和共同凭据代。监听提前建立不等于已经开始处理请求。

源码：[入口与排空](../src/smb/launch/application.rs) · [挂载视图](../src/smb/launch/custody/view.rs) · [执行同一 ELF](../src/smb/launch/custody/birth/child.rs) · [交付与确认](../src/smb/launch/custody/start.rs) · [共同放行](../src/smb/launch/pair_gate.rs)

## 3. 同一操作号怎样完成

```mermaid
flowchart TD
    Request["已授权的业务请求"] --> Check{"在原 S 核对操作号、类型、主体和规范摘要"}
    Check -->|同号但范围不同| Reject["拒绝，不改原单"]
    Check -->|已确认且当前准许查询| Receipt["读取该原单的结果"]
    Check -->|新请求或同一未决请求| Kind{"按操作类型处理"}
    Kind -->|纯 SQL 变更| Commit["短事务重核条件，一次提交业务选择与结果"]
    Kind -->|核对已有材料| Existing["事务外核对已确认材料"]
    Kind -->|需产生材料或改变位置| Pending["留下或核对待办，原单未确认"]
    Pending --> Proof["事务外准备、同步并核对原件"]
    Existing --> Commit
    Proof --> Commit
    Commit -->|已确认| Done["原决定已提交"]
    Commit -->|结果不明| CheckAgain["仍按同一操作号核对，不开新单重发"]
    CheckAgain --> Check
    Done --> Receipt
    Done -->|该单还有清理义务| Cleanup["核对已选目标和原源后按单收尾"]
```

| 实际入口 | 这类操作怎样绑定与确认 |
| --- | --- |
| Web 改共享模式，`kind=4` | 按 Web 主体、当前凭据代、`ShareId` 和前一设置核对真实变更；新模式和结果同一事务提交，不准备 FILE。 |
| SMB 保存，`kind=6` 或 `10` | 按实际 SMB 写权限、原 FILE、前一工作修订及本轮材料确认首稿或更新稿。操作号在服务端绑定；保存不发正式节点。 |
| Web 显式提交，`kind=11` | 表单固定原 FILE 和已确认工作修订。核对材料后在原 S 选择正式节点。回执查询限仍有效的原会话和原表单；查询不提交或重试。 |
| 名称或位置变化，`kind=12`、`14`、`16` | 各自冻结原父、名称、角色、位置和权限；同盘改名不是复制，回收或跨盘交接另核目标和材料。已选新位置与清源完成分别记录；普通 F 跨文件系统执行仍未放行。 |
| Helper 首设与重置 | INIT 与账户操作各有原单；需跨 SQL、卷标或私有校验材料时按各自原单完成。数据库的提交不覆盖另一个文件系统。 |

源码：[操作表](../src/system/schema.sql) · [共享模式](../src/system/share_mode.rs) · [工作修订](../src/system/work_change.rs) · [手动节点及回执](../src/system/manual_work.rs) · [FILE 交接](../src/system/file_transfer.rs) · [首设与重置契约](first-account-v1.md)
