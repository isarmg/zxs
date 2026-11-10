# 已绑定根的同步 I/O 与目录迭代器准入边界

本批接续[受控读取生命周期](guarded-disk-read-lifetimes.md)，覆盖已绑定
`DiskRoot` 的同步调用范围及返回目录迭代器的生命周期。它不是生产下线入口，
也不证明全盘已经停止 I/O。

## 本批能力

- 普通根方法先获取私有 `RootOperation`。绑定存在而准入失败时直接返回错误，
  不允许降级成未绑定访问。根目录自身的 stat、sync、available 和无需 mkdir
  的 `private_dir(root)` 也经过同一入口。
- 一次调用只有一个普通 lease。相对路径打开、递归 mkdir、metadata、读字节、
  JSON 编解码、原子写、目录同步和 unlink 都借用该操作；内部不重新获取许可。
  在开始后遇到 quiesce 的同步调用必须完成必要错误清理和已经开始的持久化尾部。
  分块读取在已有取消点返回 `DiskUnavailable`，不能把已读到内存的字节当作成功。
  普通计数只在这些栈内资源销毁后退出，不在撤销通知时提前退出。
- `open_leased_regular` 把同一操作的 lease 移入现有可撤销文件 owner，
  打开与交付之间不再获取第二份许可；若期间已撤销，文件不能逃逸注册。
- `entries` 把同一打开操作的 lease 与真实 `DirectoryEntries` 一起移入可撤销
  owner；不返回平台原始迭代器、File、FD 或可提取内部资源的引用。每次 `next`
  先在资源锁内核验准入，缓存中的名称与实际 getdents 使用同一范围。撤销会关闭
  空闲 stream，不等待调用者再次轮询；已进入的读取则保留真实 FD 和 ordinary
  计数到返回尾部。成功打开但注册晚于撤销的 stream 立即关闭并返回拒绝。
- 目录迭代器在 EOF、错误或提前 Drop 时关闭真实 stream，再释放 lease。
  EOF 后保留 wrapper 不占普通计数；错误仅报告一次，然后迭代器永久结束。
  撤销保留原平台错误形状：`Io(PermissionDenied)` 的 source 是具体
  `DiskUnavailable`，不转成可重试 Interrupted，也不把取消伪装成空目录。
  尚未绑定的 bootstrap/system 枚举仍是独立分支，绑定失败不回退到它。
  `WorkspaceInventory` 的长期 cursor 改为同一个受控类型，其他枚举调用方
  继续通过原 Iterator 接口消费；返回的名称不包含后续 metadata 或打开权限。
- `private_child` 复制原 gate。generation retention 的打开、命名地址验证和
  identity 读取使用同一操作，保留原 gate，不再经过未绑定根构造器。
  子根和持有旧根的 Arc 不能通过这些方法恢复普通准入。
- 本批仅 test 构建提供 `with_transition_io`。`begin_quiesce` 先关闭普通准入；
  `take_drained_control` 校验同一 gate 和 transition，并要求普通计数为零。
  尚有普通尾部时返回 `DiskUnavailable`，不消费控制 lease，不调用回调。
  计数为零后才消费唯一控制 lease，回调结束或失败后不可再用同一 transition
  开始另一控制作用域。没有超时或固定轮询次数替代 drain 证明。
- `RootControl` 仅暴露上限固定为 MAX_METADATA 的同步读取和原子 JSON 写入。它不暴露根、File、FD、
  PrivateDirectory、迭代器、子根或新的准入函数，没有 Clone 或布尔忽略开关。
  原有 fault `take_control` 仍只用于责任占位；它不是过渡根写入能力。

每个普通同步入口增加一次现有 gate 准入/退出和一个 lease 分配，内部嵌套调用
不增加 gate 次数。没有文件系统调用持有 gate 锁，没有改变原子写顺序、fsync
预算或既有原子文件错误来源。本批没有性能测量，不能据此声称吞吐提升。

每个绑定目录 stream 复用打开时的一个 lease，另有一个可撤销资源分配及现有
O(log 活跃资源数) 注册。每次 next 使用一次资源 mutex、准入检查和尾部 gate
检查，系统调用期间不持 gate mutex；没有新增文件同步、目录同步或全目录物化。
未绑定 stream 不增加 gate 或资源 mutex。保留原目录缓冲预算，不改变枚举顺序。

## 本阶段曾未覆盖的返回生命周期

以下保留本阶段的原始审阅范围，不代表后续生命周期整合仍使用这些原始入口。后续实际关闭与恢复边界见 [退出发布接线](disk-exit-publication.md)。无消费者的 `DiskRoot::open_optional_regular` 已删除；实际可选读取使用受管入口。

在本阶段完成时，以下接口只覆盖打开、验证及其失败清理；成功返回后的原始资源仍需迁移：

- `directory`、`open_regular` 返回的 `File`。
- `private_directory` 返回的 `PrivateDirectory`。
- 根及派生根长期持有的目录 FD。
- `lock_file` 的返回文件和已持有 writer 锁。writer 锁必须保持到认证 fence 之后，
  不能像普通读者一样撤销；本批不改其释放策略。

因此 Journal、Upload/PreparedUpload、scratch Drop、scrub、maintenance、repair，
以及调用方利用原始目录 FD 执行的 rename/link/unlink 仍不构成全生命周期闭包。
本批没有新增或迁移这些调用方的多步骤事务。普通计数为零仅表示已纳入 gate 的
责任结束，不代表上述长期资源已经关闭，也不代表现有所有业务路径均已纳入 gate。
具体而言，`business/workspace.rs` 仍将 PrivateDirectory 转为脱离调用作用域的
UploadPreparation/OpenAt2Root；`archive/persistent.rs` 的私有目录用于归档发布
和独立锁。这些能力不能通过给返回值增加可提取的包装层就视为安全，需分别迁移。

## 未绑定构造的生产调用分类

`DiskRoot::open` 和 `from_directory` 仍是显式的未绑定构造，不从路径字符串猜测
磁盘归属。现存调用尚未统一替换成独立 bootstrap/已认证磁盘能力类型：

1. 身份、冷启动及持有目录接纳：`disk/owned.rs` 的 DiskHandle open、activation、
   initialize_held、recover_held，在身份、disk.json、空目录等检查后才绑定。
2. 纯路径/独立离线工具及 payload 帮助函数：`disk.rs`、`container_storage.rs`。
3. journal 冷读、对象存储及 publication evidence：`journal.rs`、
   `journal/object_storage.rs`、`publication_evidence.rs`。
4. membership 的 pending admission、activation/recovery/publication 和 archive：
   `engine/membership_transition/pending/admission.rs`、
   `pending/activation/{recovery,publication}.rs`、`archive/persistent.rs`。
   这里既有新盘 bootstrap，也有由已有 held directory 再构造根的路径，
   必须逐个迁移，不能把此分类误读为全部只在首次启动运行。
5. 系统目录和维护辅助：`engine/workspace_storage.rs` 打开系统 workspaces 根，
   `engine/integrity_scrub.rs` 的目录地址辅助路径。

此外，自由路径函数（例如 `disk_unavailable`、seal 探测和独立 offline 工具）
并不通过已绑定 RootOperation。本批不把它们视为已有的禁止 I/O 状态加载机制。
下一阶段需要显式区分 bootstrap/system/offline 与已认证磁盘能力，迁移真实运行
调用方；绑定失败不得回退到未绑定根。本批没有修改 membership_transition 源码。

## 定向验证内容与实际状态

新增测试放在 `test/engine/unit/disk/root_operation_tests.rs`，经 owned 私有模块接线：

- 所有普通根入口、派生根和 retained 根在 quiesce 后拒绝，且不改文件或创建锁。
- 未撤销的真实同步读取返回正确值，返回前关闭文件 FD 并释放 ordinary 计数，根仍可继续使用。
- barrier 暂停真实文件读取，检查其 FD 和 ordinary 计数仍在，控制回调不能进入；
  quiesce 后读取返回 `DiskUnavailable`，observer 的具体错误不被取消替换。两种失败尾部
  都先关闭 FD、释放 ordinary 计数，之后才允许唯一控制读取。
- JSON 编码过程中启动 quiesce，后续嵌套 mkdir/原子写仍由原操作完成。
- 真实平台 AtomicFile 先 stage/prepare，再制造 no-clobber 冲突；确认临时文件
  已 unlink、原文件保留、普通 owner 未退出时仍拒绝控制操作，退出后只允许一次。
  这是已有原子清理原语的作用域测试，不是新增 syscall 故障注入或物理掉电测试。
- 另一 binding 不能消费控制能力，回调失败仍释放能力；retained identity 不符
  时释放原操作且不破坏原根。

建议 GitHub 定向执行 `storage-engine` 的
`disk::owned::root_operation_tests`、`disk::owned::directory_entries_tests`、
`disk::admission::tests`，并回归原
`disk::admission_tests` 及 HTTP/SMB 的读取撤销测试。正常与 fault 特性都需编译。

目录迭代器测试新增于 `test/engine/unit/disk/directory_entries_tests.rs`，由 owned
私有模块接线，覆盖空闲撤销真实 FD 关闭、缓存名称撤销、EOF、提前 Drop、晚注册、
打开失败、取消错误来源及单次终止、子根共享 gate、独立未绑定枚举及并发 next。
`io_admission_tests.rs` 另用 barrier 暂停真实目录读取后的 ResourceAccess 尾部，
确认关闭前 ordinary 计数不归零，尾部完成后 idle wrapper 不保留 FD。该测试使用
生产的同一资源 guard，不添加业务 hook，也不声称阻塞了内核 syscall 本身。
本切片没有新增 getdents syscall 故障注入；平台已有枚举错误语义保持不变。

本批准备阶段只做源码编辑、直接 rustfmt 和 git diff 检查。没有在云电脑运行
cargo/check/test/clippy，没有真实盘操作、部署或权限变更；测试用例尚未执行。
必须由协调者对最终提交在 GitHub 验证，不能把此文档或静态审阅当作通过证据。

主线 `f4cff1a3` 的 CI [41915719850](https://github.com/isarmg/zxs/actions/runs/41915719850)
实际报告 fencing/owned 两个旧回归仍把 quiesce 后的读取预期为成功；本轮只修正这些结果预期，
保留实际 FD、writer lock、普通责任及控制准入断言，另补未撤销读取成功的独立回归源码。
取消实现及资源关闭条件未放宽。修补尚未执行，不能把此前失败记为通过。

## 仍然阻止生产下线的条件

持久禁止状态必须在启动探测前加载，所有长期资源和清理尾部必须完成迁移；
保持服务还需要认证旧/新多数的移出票及正确 writer fencing。未满足这些条件
前不能对用户宣称磁盘已离线或零 I/O。

离线/下线应保留登记和数据，仅禁止该盘 I/O。删除登记应先在盘根持久写入可恢复
登记文件，再移除登记与独占内容的当前可见性，不擦除原文件。本批不实现删除、
搬盘、在线下线 API 或 Web 接口。
