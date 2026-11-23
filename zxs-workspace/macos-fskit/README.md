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

`RemoteDataVolume` 实现的是一个固定、可在原件上改写但不能增删条目的 FSKit 命名空间；尚未注册为已签名扩展，不能挂载。下一步的进程间登记加载须把用户所选资源和已登记本机工作身份交给本页内部 unary 回调，不接受任意路径；显示名和冲突处理属于真实 APFS 目录，人为大小写折叠不能改远端名。尚须证明：持久编辑与进程崩溃后的原所有者状态；实际访问者与禁执行/映射；独立挂载身份及正常卸载（忙时保留卷与草稿，不强制）；内核排空与无遗留写入；成功后按原选择显式提交。这些不是调用本机同步或关闭回调就能取得的证明。公开另存的 Launch Services 来源核验仍单独生效。

## 尚未运行的原机验收

[原 FD 在途 EOF／编辑器替换用例](../tests/qualification/macos_fskit_held_eof.c)没有入口点：只能由独立授权的真实 FSKit 合成卷夹具提供已核的原 FSID 与一次性文件。真实读者先打开，写者截为 5 再截为 0，分别检查新的 `fstat`、跨 EOF 短读及 0 字节；再写回 3 字节，读者必须看到正确正文。替换另测两种约定：未支持时原文件和暂存必须均不变，绝不能当编辑器保存成功；以后实现时，新开的同名项须为暂存原件，已打开的旧读者仍读旧内容。不得以用户文件作夹具。

[原生 APFS 命名用例](../tests/qualification/macos_fskit_names.swift)也没有入口：在各自新建的大小写敏感／不敏感 APFS 目录核对 A/a、NFC/NFD、恰好长得像生成别名的真实名、未登记占位和原远程名称不变。还需内核小页有／无属性分页、混用游标拒绝及 lookup/reclaim 并发验证。

另外在原系统验证：普通/其他账号、共享映射和撤权；原标记时间或相同字节替件；两份数据回执到达次序；写一半/空间不足/最后刷盘失败，之后 close 不报成功且常规卸载期间保留不确定状态；停接后已有写者完成/只读者延迟释放时不能封存；强制中断后不盲重放。未编译或执行本源码，未挂载或触碰任何设备。

### 签名宿主和正常卸载的源码边界

此内部切片已有真实的 [FSClient](https://developer.apple.com/documentation/fskit/fsclient/mountsinglevolume(resource:bundleid:options:completionhandler:)) 单入口和已绑定实例的 [FSUnaryFileSystemOperations](https://developer.apple.com/documentation/fskit/fsunaryfilesystemoperations) 回调；**没有**把它加入 ad-hoc 桌面包或开放挂载。Host 与扩展各自在自己的进程核 Apple 签名链、发布团队、bundle ID、运行 entitlement 及已配置的 [同一 App Group](https://developer.apple.com/documentation/xcode/accessing-app-group-containers)。Host 还核系统列出且由用户启用的唯一内嵌扩展。制作正式版本的团队必须合法取得签名／配置、在两个签名包中设置其真实的 `ZxsDistributionTeam` 并独立核验发行身份。离线代码检查不是公证、联网撤销检查或实际设备验收。

1. 已登记的原会话目录含单独的空 `ticket`、`names` 零字节标记目录和私有工作件。Host 仅将这个空文件作为只读的 [FSPathURLResource](https://developer.apple.com/documentation/fskit/fspathurlresource) 交给指定扩展；不交目录、凭据或正文。名字或 app-group URL 本身不是授权。加载前必须已有原卷／目录／票据实体和每件输入的登记证据；无对应原件时不按名接管，普通用户路径与强制／只读加载请求拒绝。只使用此签名 App Group 已有的访问权限，不把普通 URL 当作已授权的 security-scoped bookmark，不为其他路径申请权限。
2. Host 先保留 `/Volumes` 与原内核挂载表，独占持久写尝试标记，再以固定参数请求系统挂载。从系统返回原路径上取得 **O_EVTONLY** 根句柄，不占普通文件读写引用；核新 FSID、原卷 UUID、FSKit 标志、根节点身份、属主和 `noexec,nodev,nosuid,owners` 的实际内核状态，而非相信调用参数。核对前扩展只给绑定所需的根元数据，不给文件名或正文。不能核对时保留现场，无命令、短名、其他驱动或自动重试。
3. 正常卸载要让扩展停接新引用，等待原读写项关闭和在途回调完成，先给每件原 APFS 工作件刷盘、关闭所有写句柄；迟到写入或失败不能换取干净回执。然后只允许公开 [`funmount(原根FD,0)`](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/vfs/vfs_syscalls.c)；若忙，保留原卷等待使用者，不改用路径、不强制。成功还须从 [原生挂载表](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/mount.h) 独立确认原 FSID 和位置均消失。扩展正常停用后只在卸载资源时对私有原件作收尾，绝不在 FSVolume 停用回调中做 I/O。顺序不对的资源卸载保留原对象；确实未挂载即停用的对象可释放，但不出具干净回执。Host 不要求扩展在系统调用返回后依然活着；缺少系统或正常扩展回执，均保留未知，不认定移交或提交成功。
4. 非尝试阶段标记先同步私有原件，再在同一登记目录独占改名公布，每个标记绑定原票据、卷、阶段及自身创建身份；不覆盖碎片或扫描其他会话。它们只证明本机生命周期；不能充当已封存字节、R3 身份或远端收据。

[无入口的宿主源码用例](../tests/qualification/macos_fskit_host.swift)仅定义错资源、未明确授权和双回执断言；没有运行。在明确批准的真实 macOS 27 参考机，仍须独立验证：实际签名／entitlement 和开启流程、空只读票据搭配可写卷、FSClient 选项实际传递、根事件 FD 是否不阻塞正常卸载、跨用户／可执行／映射、双进程崩溃与忙卷收尾。须将真实 ZXS 选择登记为原 App Group 工作件，接入签名扩展入口、在进程间核原始登记，移交后复核最终文件代次及整份摘要，才可与已实现的原号提交相接；普通 Qt 发布入口仍关闭。

### 两个进程如何认回同一批已选件

[真正的扩展目标与静态宿主桥](target/README.md)是可选源码，不改变普通桌面包。桌面先用签名宿主查系统是否确有已启用的同团队原扩展；不满足就不下载正文。用户明确选择后，[登记器](../zipworkspace/macos_fskit_registration.py)固定原连接／凭据所属账号、容器和分支／头／节点，从真实 R3 详情和历史两端核验，经同一 HTTPS/2 原节点 GET 写入 App Group 独占新 0700 会话内的生成私有件，再用真正独立只读句柄核全长和 SHA。每次准备有预先分配的 token，仅一次；中断保留原现场，不自动扫描、清理、挂载或改服务器。不是公开的 Finder 另存。只把当前主可执行文件中静态链接的受权桥用于打开 OS 提供的固定组目录，不接受调用方或环境变量给的路径。

宿主给原 `host-attempt` 同时绑定独立持有的规范登记 SHA、空票据/目录身份和随机卷 UUID。`RemoteExtension` 真实入口在另一个进程使用自己的有效签名、同一专用组和这个持久证明，只取准确命名、0400、受限大小、无 ACL、不可替代的 `projection.json`。初始探测不打开任何正文、不建名；系统加载先独占占用原登记，再从逐文件原卷/实体/创建时刻、确切长度/代次/完整 SHA 构造 FSItem。仅此真实加载对可见名进行 APFS 碰撞处理。有效组内发布者和 OS 对组数据的保护是信任边界；普通 UNIX 权限不防同一用户另行给予其他应用访问。

[桌面桥](../zipworkspace/desktop/macos_fskit_host.py)把一次挂载请求排队，区分尚在挂载、已核挂载、停止、忙、正在正常卸载、只有内核确认、两边均确认与结果未知；普通内核卸载不在 GUI 主线程等待。此切片仍不把本机正常收尾当远程提交或最终字节证明。尚须原所有者最后代次/SHA 对接已有私有待提交包、普通面板接入和真实签名/驱动验证；没开放生产挂载。登记解析/替换断言见[无入口源码用例](../tests/qualification/macos_fskit_projection.swift)；本次未编译、签名、执行或挂载。
