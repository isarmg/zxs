# ZXS 术语

以下以当前 SQLite 池和前台启动为准。

| 中文 | English | 在 ZXS 中指什么 |
| --- | --- | --- |
| 原单 | Original operation | 同一池第一次接纳的规范请求及其 `OperationId`。用类型、主体和请求摘要核对。新请求不得借用旧号。SMB 报文的 `MessageId` 不是这个操作号。[账本](sqlite-system.md#操作与故障点) |
| 原物 | Original filesystem object | 该登记或本次操作实际持有的目录或文件，例如 SYSTEM 根、某个 FILE 或它的材料。按用途核原路径、挂载和同一实体；冷重开还须核对持久证据。同名或同内容不足以替代它。[实际根与锁](sqlite-system.md#打开只读与独占维护) |
| 原父、原名 | Bound parent and name | 当次操作绑定的父目录和按原字节比较的名称。逻辑父用 `DirectoryId` 或明确卷根定位，还须核实际父目录。改名或换父可以变更位置，不重新分配 `FileId`。[文件命名](storage-names.md) |
| 原决定 | Committed business decision | SYSTEM 中已提交的业务选择，例如共享模式、已确认工作修订、正式节点或 FILE 唯一位置。占位待办、普通文件已写入、物理改名已发生，都还可能处于待确认阶段。[操作与故障点](sqlite-system.md#操作与故障点) |
| 原站点 | Selected installation | 由 `/zxs/state/deployment.v1` 选中的实例。绑定原 `site.v1`、安装描述、SYSTEM 和批准的同一 ELF；不是 HTTP 地址或主机名。[站点契约](../src/smb/launch/site/CONTRACT.md) |
| 原认领 | Original volume claim | SQL 的 `volume_claim` 行与实际卷根上与它相符的卷标；包含池、卷、角色、初始认领操作及认领标识。卷标不能自行选择池，使用时还要核实际介质。[数据归属](sqlite-system.md#谁保存什么) |
| 原回执 | Original operation receipt | 该原单的已确认业务结果，例如原工作修订、原版本或原目标。按同一号、主体和规范输入、在当前允许查询的条件下读取；不是当前文件位置或内容权限。[操作路径与回执](architecture-maps.md#3-同一操作号怎样完成) |
| 凭据代 | Credential epoch | 共同账户当前生效的凭据操作及代次。Web、SMB 分别认证，重置后旧代不能当作当前授权。[共用账户](first-account-v1.md) |
| 放置代 | Placement epoch | 某一 FILE 已选位置的代次。移动或还原会改变位置，`FileId` 和原历史仍保留；旧位置不能凭旧代重新打开。[回收与还原](recycle-and-restore.md) |
