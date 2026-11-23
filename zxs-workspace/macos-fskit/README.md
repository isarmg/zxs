# macOS 可写远程卷：FSKit 文件与目录源码

这一目录只有一个生产方向：Apple FSKit，不增加 macFUSE 可写后端。现有 macFUSE 合成只读会话和普通 ZIP 不变。桌面归档仍面向 macOS 26；可选可写卷以 **macOS 27、相应 Xcode SDK 与 Darwin LP64** 为下限，原因是需要 [FSContext](https://developer.apple.com/documentation/fskit/fscontext) 和能对每次打开明确授予“不缓存”的 [FSVolume.DataCacheHandler](https://developer.apple.com/documentation/fskit/fsvolume/datacachehandler)。省略该协议并不表示内核不缓存。

## 目前能说明什么

- `RemoteDataVolume.swift` 与目录接口实现了真实的 [FSKit 文件读写](https://developer.apple.com/documentation/fskit/fsvolume/readwritehandler)与缓存协商回调，不是仿造系统结构。`WorkFile.swift` 只接控制器登记的原私有工作目录、生成的容器 ID 和 APFS 物理证据，借独立读、写原件；既不保存凭据也不联网。两次打开都在一次性原生线程上关闭自动物化，校验实存 0600、原名、原卷 UUID、实体、纳秒创建时刻、0700 父目录和无 ACL；生效时匹配原长度／修改时间／元数据时间，再读完整件核对原摘要。已驻留的工作目录须是本应用独占保管的本机非同步目录，不支持 File Provider、同 UID 绕过写入或原生文件被后台驱逐。卷内属性是虚拟普通文件的属性，不是来源证明。
- 每个容器在本卷内绑定一个独立的 FSItem；打开／升级核真实及有效用户、组，只授 `.noCache`，禁内核绕过数据面。多个读/写请求在同一原文件上串行，重叠被明确转发的写打开拒绝；FSKit 不给读写回调逐用户 FD，不能声称这是任意进程之间可强制的独占写锁。系统[最后一次回调](https://developer.apple.com/documentation/fskit/fsvolume/datacachehandler/close(_:context:replyhandler:))是一个对象所有引用结束，并非每个应用 FD 的关闭。错误不能从这个回调返回，更不代表正常卸载。
- 单次数据请求最多 16 MiB、单文件最多 128 GiB。读按当次锁内的真实 `fstat` 长度裁剪，跨 EOF 返回实际字节数，达到或超过 EOF 返回零，不补零。写后和截短后在同一锁中重新取大小及带序号的属性；FSKit 要求但此数据面不支持的字段不编造。仅支持按已认证项改大小；不模拟属主、ACL、标志位、时间、临时改名或替换操作。只关闭本机原写句柄前必须停止新打开、收到最后引用通知且同步成功。短写、同步不确定或成功改动后构造系统回执失败会锁住后续同步/移交；普通关闭不生成版本。
- `WorkEvidence.h` 是可选扩展目标的桥接头。`WorkEvidence.c` 使用 Darwin SDK 的实际结构，从原句柄读取权限和 [APFS 身份/准确拼写](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/attr.h)。本方重复持有原目录句柄；协作式文件锁不代替主机的原卷所有者租约。上述本机收尾后不是远端提交；要接入[已有完整正文与原号恢复](../docs/macos-remote-edit.md)，还须跨正常卸载到整份正文移交一直保留与实际原卷绑定的所有者和源代次，不准按文件路径重新选择。

## 本机目录及正常收尾

- `NameIndex` 接受已登记的同一 APFS 卷上新建、空、0700 且由应用独占的命名目录，及 1–128 个已核的工作文件。目录只放独立的零字节标记，正文仍在各自生成的私有工作文件中。查询实际 APFS 的有效大小写、保留拼写能力；按远端原名 UTF-8 字节和容器 ID 排序，先尝试所有原名，再对原生新建确实冲突的项目分配“名-完整 ID[-序号].扩展名”。不转小写、不做 Unicode 归一化、不覆盖／删除原名或不明占位，也不写回远端名称或改变 ZIP 规则。非法／此 APFS 无法创建的名称明确失败。失败后的目录不得自动接管复用。
- [FSVolume.Handler](https://developer.apple.com/documentation/fskit/fsvolume/handler) 报告原生大小写语义。查找由刚才那个 APFS 目录执行，核原标记实体及实际保存的字节拼写再返回。按 FSKit 要求分别处理无属性（含两个点项）与有属性（不含点项）的枚举；续读游标只在原目录、原模式下有效。标记由同一个资源所有者保留，核所有者与引用后允许 FSKit [回收临时的内核引用](https://developer.apple.com/documentation/fskit/fsitem/tryreclaim(_:))，不关闭正文。只允许现有文件内容读写及明确改大小，目录新建、删除、链接和编辑器临时替换均明确拒绝。不能把就地写回支持说成各种编辑器均能保存。
- 扩展侧按激活、挂载、停止新打开、正常卸载通知、停用依次转移；同步全部原工作件，具体错误一直保留。在所有对象确已排空之前不关闭任何写句柄。即使这些回调均成功，仍**不能**据此判断原卷已在系统卸载或允许远端提交。意外/强制断开不作为成功，未挂载的激活可释放而不授权移交。

## 生产挂载的前提（入口未开放）

仅选 Apple [FSClient.mountSingleVolume(resource:bundleID:options:)](https://developer.apple.com/documentation/fskit/fsclient/mountsinglevolume(resource:bundleid:options:completionhandler:))。它指定扩展 bundle ID，原系统在 `/Volumes` 创建挂载点。正式宿主必须由实际签名和配置授予 `com.apple.developer.fskit.mount`；扩展须有 [FS 模块 entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.fskit.fsmodule)。由用户另行安装、在系统设置中启用。宿主需检查系统列出的模块、启用状态及预期签名，核已登记工作资源；不能把短文件系统名、URL 字符串或自己的测试开关当身份。现有 ad-hoc Qt 包没有这些资格。不调用安装程序、不打开权限设置、不降级到命令、短名或内核驱动，不强制卸载。

`RemoteDataVolume` 实现的是一个固定、可在原件上改写但不能增删条目的 FSKit 命名空间；尚未注册为已签名扩展，不能挂载。资源加载器尚须从用户授权资源和事先登记的本机工作身份取得原句柄，不接受任意路径；显示名和冲突处理属于真实 APFS 目录，人为大小写折叠不能改远端名。尚须证明：持久编辑与进程崩溃后的原所有者状态；实际访问者与禁执行/映射；独立挂载身份及正常卸载（忙时保留卷与草稿，不强制）；内核排空与无遗留写入；成功后按原选择显式提交。这些不是调用本机同步或关闭回调就能取得的证明。公开另存的 Launch Services 来源核验仍单独生效。

## 尚未运行的原机验收

[原 FD 在途 EOF／编辑器替换用例](../tests/qualification/macos_fskit_held_eof.c)没有入口点：只能由独立授权的真实 FSKit 合成卷夹具提供已核的原 FSID 与一次性文件。真实读者先打开，写者截为 5 再截为 0，分别检查新的 `fstat`、跨 EOF 短读及 0 字节；再写回 3 字节，读者必须看到正确正文。替换另测两种约定：未支持时原文件和暂存必须均不变，绝不能当编辑器保存成功；以后实现时，新开的同名项须为暂存原件，已打开的旧读者仍读旧内容。不得以用户文件作夹具。

[原生 APFS 命名用例](../tests/qualification/macos_fskit_names.swift)也没有入口：在各自新建的大小写敏感／不敏感 APFS 目录核对 A/a、NFC/NFD、恰好长得像生成别名的真实名、未登记占位和原远程名称不变。还需内核小页有／无属性分页、混用游标拒绝及 lookup/reclaim 并发验证。

另外在原系统验证：普通/其他账号、共享映射和撤权；原标记时间或相同字节替件；两份数据回执到达次序；写一半/空间不足/最后刷盘失败，之后 close 不报成功且常规卸载期间保留不确定状态；停接后已有写者完成/只读者延迟释放时不能封存；强制中断后不盲重放。未编译或执行本源码，未挂载或触碰任何设备。
