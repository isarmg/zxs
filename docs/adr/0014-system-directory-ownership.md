# 0014：SYSTEM 持有目录与 SQLite 文件系统边界

状态：候选设计，尚未完成全流程接线与验收。现有显式 data-only 恢复补丁不能据此视为安全完成。

## 背景与已确认边界

[同代磁盘资源所有权](0007-owned-disk-resources.md)已把业务盘 I/O 绑定到持有目录，
但 SYSTEM 仍有路径消费者。恢复入口逐分量检查一次 SYSTEM、检查配置不存在，
或在写入前比较 device/inode，都不能保证之后的绝对路径访问不会跟随新祖先。
`0700`、writer flock、纯词法互斥检查也不能代替这个所有权约束。

当前需要迁移的主要边界：

| 消费者 | 当前性质 | 所需边界 |
| --- | --- | --- |
| `engine.rs` 初始化、普通冷开、显式恢复 | 准入后仍传递 SYSTEM `PathBuf`，配置与工作区可重新按路径访问 | 从首次准入到最终 Engine 保留同一目录 owner |
| `engine/config.rs::acquire_locks` | `writer.lock` 用绝对路径 `open`，NOFOLLOW 只限制末分量 | 相对同一 SYSTEM owner 打开、锁定和核对命名 inode |
| `engine/disk_lifecycle/terminal/store.rs` | terminal 打开已持有 `DiskRoot`，但锁和重新恢复的 SYSTEM owner 还没有统一移交 | 保留现有 held 操作，禁止恢复边界重新选择目录 |
| `engine/index.rs`、`index/snapshot.rs` | SQLx、main/WAL/SHM 检查及 quarantine 仍用路径 | 全部数据库访问、辅助文件和快照共同持有同一能力 |
| `engine/workspace_storage`、checkpoint、membership/lifecycle 的 SYSTEM 投影 | 部分调用各自重开 SYSTEM 或子目录 | 只接收同代 owner 和受限相对名称 |

这是一份实现接线清单，不把同名 `root` 或 `join` 的静态搜索结果全部当作漏洞；
业务盘路径已经交给 held owner 的调用不应退回路径实现。服务端与离线工具的 SYSTEM
消费者也须逐调用链检查，不能以 engine 编译通过推定所有入口完成。

## 决策方向

1. 建立私有构造的 SYSTEM owner，持有已准入目录和仅供诊断的路径。普通冷开、
   初始化、管理恢复、工作区 inventory 共享这一入口，不提供由路径隐式重开的转换。
   已存在 SYSTEM 在第一轮准入后持续持有；缺失 SYSTEM 保持缺失，直到已有恢复
   权威、显式数据身份和配额检查允许创建，再消费逐分量安全创建得到的目录。
2. writer 锁与 owner 一起移交。打开、类型/单链接/权限检查、flock、命名 inode
   复核和既有同步均使用同一目录 FD。长期 worker、管理 session 和只读索引
   snapshot 保留相同所有权，不能在锁仍存在时把 SYSTEM owner 换成新目录。
3. SYSTEM 子目录与文件操作接收 `EntryName` / `RelativePath`，由 held owner
   执行受限相对解析。路径只能用于展示、持久化定位提示及显式名称绑定核验。
   需要“命名目录仍是原目录”的操作仍检查该策略，但核验后的 I/O 不得重新按路径打开。
4. SQLite 接口必须表达并保留目录能力，不能返回一个可能比目录 owner 活得更久的
   裸连接。显式 close 必须等待 native worker 及其所有 sidecar 访问结束；取消、
   初始化失败、关闭失败和 Drop 同样不得提前释放能力。当前 FULL、WAL、defensive、
   原生限额、关闭错误来源和禁止隐式 checkpoint-on-close 的职责保持不变。
5. SQLite 的文件系统实现必须覆盖 main、WAL、rollback journal、SHM、目录同步、
   access、delete、rename/隔离及延迟打开。每项访问都要在同一 held 目录内解析；
   链接、特殊文件、错误所有者/权限与多链接不得因数据库是可重建缓存而放行。
   ATTACH、临时文件、扩展加载和未知文件名必须有明确范围，不能成为逃逸出口。

这是所有权与访问方式改变，不改变 quorum、日志权威、提交顺序、现行磁盘格式，
也不增加旧格式兼容、迁移、池重置或部署授权。

## SQLite 不能采用的捷径

本地 [SQLite amalgamation](../../src/foundation/data-storage/libsqlite3-sys/sqlite3/sqlite3.c)
的 Unix VFS 在 `unixFullPathname` / `appendOnePathElement` 中解析符号链接。
把 `/proc/self/fd/<directory-fd>/index.sqlite` 直接传给当前 SQLx adapter，
会重新得到可替换的命名路径，不是持有目录实现。

仅包装 `xFullPathname` 或 `xOpen` 也不足以证明完整性：`unixOpenSharedMemory`
从保存的数据库路径构造并直接打开 `-shm`，并非普通 VFS `xOpen` 调用；Unix 层
还有 `findCreateFileMode`、`findReusableFd`、`unixAccess`、`fileHasMoved`、
删除和父目录同步等路径访问。当前 `O_NOFOLLOW` 限制末分量，不能保护祖先。
同一进程中为检查 SQLite 文件而额外 open/close 原始 FD，还必须评审 POSIX
锁的进程级释放语义，不能随意增加“验证性”打开。

采用新的 held-directory VFS 前，须单独审阅上述全部行为和 fd 生命周期、
跨连接锁、SHM 共享、mmap、失败关闭及 FFI 安全。不能只按默认 VFS 复制函数指针，
或全局替换 Unix 系统调用回调，假定其它并发连接不受影响。实现未完成时保留
当前功能边界与明确阻断，不启用依赖该证明的新恢复能力。

## 可独立落地的现有原语修正

`PrivateDirectory::verify_identity` 的命名核验使用现有
`DIRECTORY | NOFOLLOW` 逐分量打开，再比较持有目录的 device/inode 和权限。
旧的整路径 `symlink_metadata` 只不跟随末分量，会接纳指回同一 inode 的祖先链接。
新检查拒绝这种别名，也保留不存在、ENOTDIR/ELOOP 等具体 I/O 来源。

它仍只是本次名称绑定核验，不锁住命名空间，不保证检查后的路径不变，也不完成
上表的 SYSTEM 或 SQLite 迁移。相对原语依旧绑定原 FD；名称验证失败不删除、修复
或更改目标权限。深度 d 增加 O(d) 次目录打开，额外同时存活的句柄为 O(1)，
不增加 fsync；不能描述为性能优化。

## 接受所需证据

- 确定性交错覆盖 SYSTEM 叶子、任一祖先被重命名后替换为普通目录、符号链接、
  指回原 inode 的链接和悬空链接。分别在初次准入、取得锁前、连接创建、第一次
  WAL/SHM 使用、checkpoint、只读 snapshot、quarantine、close 中触发。
- 对未授权候选记录 syscall 证据，拒绝窗口内零候选 stat/open/write；有真实正校准。
  “文件未变”“返回 UnsafePath”或检查完再 stat 都不能代替零探测证据。
- 数据库锁和 owner 的并发/释放测试：多个连接、保留 snapshot、取消、初始化失败、
  关闭失败、跨进程锁竞争、最终 FD 关闭。不得以单连接 happy path 代替。
- 保持损坏索引隔离与所有 journal 的顺序、命名 inode 对应、无覆盖发布及目录屏障；
  正常恢复、重建和只读退化仍服从原契约，原始权威和候选字节完整保留。
- 当前 Rust 目标与真实依赖特性组合测试、定向 strace、故障注入分别报告。
  平台原语回归不代表组合恢复、生产并发或物理断电验收通过。
