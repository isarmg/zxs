# 存储目录候选发现

`src/server/src/storage_directories.rs` 是只读目录边界，wire 类型位于
`storage_contract::storage_directories`。它不创建目录、改权限、写探测文件、
挂载、卸载、格式化或清除数据。管理面只发现 systemd/CLI 固定父目录
下的直接子目录，不扫描整机或递归搜索，也不要求部署时预先枚举候选白名单。
HTTP 管理面必须自行保留当前管理员认证、请求预算和后台任务所有权；
模块本身不是公开匿名文件系统接口。

## 路径与浏览

内部底层 `browse(path)` 接受绝对、规范化 UTF-8 路径，最多 4096 字节；拒绝 NUL、
`.`、`..`、重复斜杠和非根路径的尾斜杠。每一组件不跟随符号链接。
返回实际 `statx` 挂载 ID 对应的 mountinfo 记录，不按路径前缀猜测所属磁盘。
管理面入口必须使用下文的 `browse_within` 限定发现根。范围内的子挂载
按其自身身份解析，不作为原挂载里的同一对象。

只列目录及不可跟随的符号链接，不返回文件正文。最多返回 200 项、扫描
4096 项；超出时 `truncated=true`，不是完整清单或快照。无法用 UTF-8 表达
的名字计入 `skipped_non_utf8`，不伪造可使用路径。目录条目的
`can_browse` 只表示本次可导航，不表示可初始化。权限失败或链接明确标记。
mountinfo 读取上限为 4 MiB / 4096 条，解析失败不猜测。

清单区分普通文件系统目录根与 `nsfs` 命名空间文件根。Linux 的
[`nsfs_show_path`](https://github.com/torvalds/linux/blob/master/fs/nsfs.c)
会输出 `net:[inode]` 等非路径标识；Docker 的 host-network 默认 namespace
绑定可使这类记录出现在宿主清单中。它们保留挂载 ID、设备号、挂载点和完整
namespace 标识，仍计入相同条数上限并参与清单变化比较，不让无关 namespace
记录阻断正常目录发现。namespace 文件不能成为存储候选或文件系统路径别名；
涉及其挂载点的路径映射明确拒绝。普通目录根仍严格验证绝对规范路径，
格式错误、未知转义、重复挂载 ID 与超限清单仍失败，不跳过错误记录。

`cgroup` / `cgroup2` 的根也作为独立非存储标识保存，包括普通 `/` 根。
Linux [`cgroup_show_path`](https://github.com/torvalds/linux/blob/master/kernel/cgroup/cgroup.c)
按读取者的 cgroup namespace 根调用
[`kernfs_path_from_node`](https://github.com/torvalds/linux/blob/master/fs/kernfs/dir.c)，
可合法返回 `/..`、`/../..` 或 `/../../sibling/child`。解析器只允许开头连续
父组件及其后的普通组件，保留完整标识、文件系统、挂载身份、顺序和预算；
不将它规范化为 `/`，也不跳过该记录。cgroup 根不能参与目录别名映射、
发现根或存储候选，返回实际 `cgroup` / `cgroup2` 的不支持文件系统错误。
普通文件系统的父组件、非绝对挂载点、错误转义及超限仍严格拒绝。相关单元
夹具覆盖清单保留和映射／候选拒绝，不等同于特权 cgroup namespace 挂载验收。

## 检查与激活边界

`inspect(&InspectRequest, excluded_roots)` 要求 `mount_path` 为实际挂载点，
`root_path` 可以是挂载点下的任意合法空子目录。两者不是同一个字段。
初始化目录沿用核心私有目录契约：服务账号所有、精确 0700，实际读取、
写入和搜索访问检查通过，且当前为空。明显的内核控制和内存文件系统
（如 proc/sysfs/tmpfs）不能作为存储候选；不因未知设备或文件系统名就
宣称它是独立物理磁盘、持久性已验证或未来写入一定成功。

`excluded_roots` 必须由服务器形成，包含系统目录、管理目录、已登记根及
同批其他候选。双向祖先重叠均拒绝。现有根按实际设备号和 mountinfo
filesystem-root 映射检查 bind-mount 别名；未创建的排除目录使用最近现有
祖先和剩余路径，不能仅因该目录尚不存在而忽略重叠。未能可靠观察排除
目录时拒绝，而非当作不存在。

选择结果绑定挂载 ID、设备号、挂载根 inode、文件系统及其内部根，以及
所选目录的 device/inode/ctime。`selection_id` 为该结构的内容摘要，不是
秘密、签名或授权凭据；提交时不信任客户端宣称的设备身份。

`revalidate(&selection, excluded_roots)` 再次执行真实检查、比较完整选择，
返回持有挂载和实际目录描述符的 `HeldSelection`。执行器必须消费
`into_storage_directory()`，不能检查后又按 `root_path` 重开写入到替身。
执行器还须在整个激活集合的协调范围内再次校验排除根、空目录和配置准入。
本模块不能防止其返回后其他进程修改权限、空间或目录内容。

挂载 ID 不是跨重启的持久身份；ctime 仅用于短期陈旧检测，正常初始化也
会改变它。持久化意图与恢复必须使用配置、核心 disk seal 和实际目录
身份证明，不能仅凭本次发现摘要重新激活目录。

## 验证边界

聚焦测试在临时私有目录中覆盖空子目录、实际描述符身份、链接、路径
格式、非空和权限拒绝、目录替换、选择/挂载身份篡改、原数据不变、
有界清单、wire 整数表示与未知字段、bind-mount 根映射计算。

映射及挂载身份变更的单元用例不等于在真实内核执行挂载切换。本文不
宣称完成热拔盘、特权 mount namespace 故障注入、浏览器或激活端到端验收。

## 管理面限定发现根

`--storage-discovery-root` 默认 `/zxs/data/disks`，由 systemd/CLI 启动配置固定。
`DiscoveryRoot` 在启动边界解析为绝对规范路径，可接受一个结尾 `/` 并规范化；
不创建或探测该路径。`GET /api/admin/storage/discovery-root` 只报告配置，
旧 PUT 和网页路径编辑已移除，不读取旧管理目录发现设置作为授权来源。
缺失根如实报错，不创建、不退回 `/`。本服务不挂载设备。

父目录只作为容器，自己的内容不能成为新磁盘。`browse_within(root,path)`
仅接受 `path == root`；`inspect_within` 和 `revalidate_within` 仅接受直接子目录。
例如父目录 `abc/` 中 `a/`、`b/`、`c/` 是三个候选，不发现 `a/nested/`。
内部受控测试仍可用底层 `browse/inspect/revalidate`，HTTP 不能绕过固定根。
实际挂载点可能在父目录之外，保留它作身份校验，不把它变成新的浏览范围。

`GET /api/admin/storage/discovered-disks` 不接受查询参数，按 held parent FD
只读列举直接子目录。结果含 `empty`、`zxs`、`unsupported`、`unavailable`：
只有通过空目录检查的 `empty` 携带初始化 `CandidateSelection`；其他项没有
空目录写能力。ZXS 识别读取本盘 seal、固定配置锚或代际证明，不打开其他盘，
返回记录的级别及证据强度；`initial_declaration` 只是身份一致的本地声明，
`generation_evidence` 也不证明完整业务历史、最新 HEAD 或迁入权限。
损坏、不支持或物理身份变化的盘明确拒绝，不能默认为一级或降级成普通导入。

候选展示在磁盘树的 -1 层，不进入已接纳拓扑，不画备份关系，也不自动写入。
已登记、停用以及系统/管理目录在打开子项前排除，包含其挂载别名；临时缺失
的已登记路径仍排除，不能视为新空盘替换。冷启动缺少已准入历史权威时先拒绝
发现，不能因 Engine 尚未打开就忽略已知成员。

列表仍受 200 返回项、4096 扫描项预算约束并报告截断。`discover_empty` 是
独立只读筛选器，清单截断时整次拒绝，不自行挑选前几项初始化；产品没有
后台自动初始化循环。普通转换扫描是用户进入向导后的独立工作，不是递归发现。

来源 `DirectorySource` 只绑定路径、parent 身份和自身身份。`reopen_source`
从固定 parent FD 重开并复验这两层观察，排除已知成员、停用根及新目标的
真实文件系统别名，不授予写权限。转换还需完整源清点、确认摘要和原操作。
两种选择均含 device/inode/ctime；更换父目录、条目或其短期状态使旧预览失效。
已开始激活的恢复则使用原持久意图、配置和核心 seal，不以重启后的 mount ID
或改变的 ctime 重新选择目录。

## 普通来源转换

已有活动系统可通过[普通盘转换协调器](../../../design/online-disk-management.md#ordinary-disk-conversion)
把普通来源复制到另一直接空目录并接纳为新的一级盘。它保留来源，分开记录
成员接纳和根内容发布，只有第二阶段的永久原回执才能显示整体完成。外来 ZXS
的可移植身份、完整历史闭包和来源分组重建尚未由此实现，不开放伪成功入口。

外来 ZXS 另有显式只读 `/api/admin/storage/foreign/preview`，使用同一来源观察
与已登记根排除规则，核验本盘可读取的发布闭包并报告原一级来源和正文暂不可用
状态；它不执行迁入、剪裁或初始化。详细预算、证据强度及多来源连通历史的
拒绝边界见上述在线管理设计，不能把检查通过当作写入准入。
