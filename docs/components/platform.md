<a id="platform-io"></a>

# Linux 平台基础（platform-io）

ZXS 独立的 Linux 服务端基础库，在本项目维护和构建。

| 模块 | 本项目职责 |
| --- | --- |
| `fs_safety` | 持有目录句柄、拒绝链接和路径逃逸、单链接文件、文件及目录同步、无覆盖发布 |
| `payload` | 有长度上限的完整流式写入、BLAKE3 校验、新 inode、不可覆盖发布 |
| `sqlite` | SQLx 0.9 索引连接、WAL/FULL/外键、defensive、原生连接限额、完整性及快照检查 |
| `schema_identity` | 索引表结构身份和确定性指纹 |
| `secret` | 默认脱敏、不可序列化、释放时清零的秘密类型 |
| `error` | 有长度上限的错误和请求标识；支持 ZXS 大写业务错误码 |

`sqlite::block_on_sqlite_connection` 支持同步核心执行直接 `SqliteConnection` 操作；不支持连接池、网络、Tokio 定时器或递归调用。每次必须在 future 返回前显式 `Connection::close().await`。`open_index_connection` 固定索引连接策略并启用 defensive；`create_if_missing=false` 不允许隐式新建数据库。

平台库不拥有数据盘路由、产品账户状态、产品 DDL 或跨盘事务真相。SQLite 仅为可重建索引。`payload::Receipt` 证明单文件落盘，不能被当作 ACCEPT、FINALIZE 或 PUBLICATION。

认证算法的唯一实现和对应回归测试在 [authentication](auth.md)。该独立 crate 不依赖 Linux 或 SQLite，服务端和救援工具直接依赖它；平台层不保留兼容导出，避免同一密码/令牌策略维护两份实现。

本项目定制包括：

- 所有索引池连接和直接索引连接在处理业务语句前启用 SQLite defensive。
- 错误码允许全大写或全小写，拒绝混合大小写，避免客户端歧义。
- 完整 payload 使用固定 64 KiB 缓冲区，不使用硬链接、reflink、稀疏复制或去重；摘要或长度不匹配时清理暂存文件。
- 存储核心和运维 CLI 使用此 crate 的 Linux amd64 本机能力；远程管理客户端 storage-client 和浏览器 Web 不引入 Linux 盘操作及 SQLite 本地 FFI。原生产品和构建宿主的支持范围仍仅为 Linux amd64。


平台库的 Tokio 直接生产依赖仅启用 `rt`；SQLite 依赖自行声明所需的 `sync`/`time`。测试单独启用宏和多线程运行时，以覆盖两种执行环境。这不会移除服务端实际需要的多线程运行时，也不改变 SQLite 持久化策略。

## 私有 payload 打开模式

`PrivateDirectory::open_payload` 显式接收 `PayloadOpenMode`：`CreateNew` 使用
`CREATE | EXCL` 排他创建，目标已存在即失败；`Existing` 要求目标已存在。
两种模式都保持可读写、不截断、非阻塞、拒绝符号链接，并在打开后检查普通文件、
单链接、当前服务所有者和精确 `0600` 权限；系统调用错误保留来源。
操作仍锚定持有目录 FD，目录路径被换绑时不转向新目录；提交前另由
`verify_payload` 检查目录和文件身份。HTTP 首次及新代上传使用 `CreateNew`，
同代重试使用 `Existing` 并核对全部物理前缀后续写；SMB 写副本始终排他创建。
模式不提供隐式补建、截断或耐久提交，不改变既有同步、预算及取消所有权。

## 原子写屏障所有权

`AtomicFile::create` 与 `replace` 都在本次独占临时文件上完成写入和
`File::sync_all`，再修改目录项，核对发布 inode，最后同步持有的目标目录。
create-only 在发布前保留单链接检查及临时名称到持有 inode 的对应检查，
只复用私有 `rename_no_replace` 目录项步骤，不重新打开并同步同一临时文件。
公共 `NoClobberPublish::publish` 接受未知持久性的 staging，仍独立完成打开、
单链接准入、文件同步、NOREPLACE、发布身份检查和目录同步。
三个入口都只接受同一持有目录内的单个名称，不新增跨目录发布能力。

rename 前失败保留原目标；内部临时文件由清理守卫回收，公共发布的 source
由调用方拥有。rename 成功即解除临时清理；之后目录同步失败返回
`PublishedDurabilityUnknown` 并保留已发布目标，不伪称回滚。replace、权限、
现存对象重试屏障、锁文件与祖先目录持久化协议不变。

普通 fresh create 的静态预算从两次文件同步加两次目录同步降为各一次
（不计创建祖先目录）；这不是端到端性能实测。故意仅在被删除的第二次同步
注入 EIO 时，错误表现当然不再逐调用相同；耐久顺序仍为文件先于 rename、
目录后于 rename。根测试 `test/platform/unit/fs_safety/atomic-io.sh` 接受已编译的
platform-io libtest 二进制，隔离运行 create/replace/public 的成功、写失败、
文件同步失败、真实发布冲突和目录同步失败；断言命名空间、源保全与清理。
replace 的目录冲突由 rename 前类型检查拒绝，create/public 由 NOREPLACE 拒绝；
这不冒称每种发布冲突都到达了 rename 系统调用。
其 LD_PRELOAD 日志只覆盖 `std::fs::File` 的 libc write/fsync 路径，检查文件屏障
时目标尚非该 inode、目录屏障时已存在目标，不声称拦截 rustix direct syscall、
全进程 syscall 总数、精确 rename 时间或物理断电行为。
