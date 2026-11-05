# 存储目录候选发现

`src/server/src/storage_directories.rs` 是只读目录边界，wire 类型位于
`storage_contract::storage_directories`。它不创建目录、改权限、写探测文件、
挂载、卸载、格式化或清除数据。管理面只发现默认或管理员明确设置的发现根
内部子目录，不扫描整个文件系统，也不要求部署时预先枚举候选白名单。
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

管理面默认 `DEFAULT_DISCOVERY_ROOT = "/zxs/data/disks"`（复数），也可由
当前管理员显式设置其他发现根。设置属于系统控制面持久状态，不接受
浏览或检查请求自行声明新的授权根。`DiscoveryRootSettings` 与
`SetDiscoveryRootRequest` 拒绝未知字段；更新请求带 `expected_discovery_root`
供设置所有者作并发比较，不能悄悄覆盖另一项已完成的设置。

`validate_discovery_root(root, excluded_roots)` 只验证已经存在、无符号链接、
具备实际读取/搜索/枚举权限且不重叠系统或管理根的目录。发现父目录不
要求 0700，不创建缺失目录、不改权限。真正存储候选仍须通过原有私有、
空目录与挂载检查。默认路径不存在时应如实显示不可用，不能创建它或退回 `/`。

HTTP 浏览只可调用 `browse_within(root, path)`。根本身可以作为列表容器，
只列其内部真实目录，不列普通文件或符号链接，也不扫描其他文件系统目录。
可以继续进入合法子目录和其内部实际子挂载。路径组件前缀只是初筛；
真正目录由持有的发现根 FD 通过 `openat2 BENEATH/NO_SYMLINKS` 打开，
条目可访问性探测也相对于所列目录 FD，不通过可竞态替换的绝对子路径重开。

`inspect_within(root, request, exclusions)` 只接受发现根的严格后代；发现根
自身不能作为候选。`mount_path` 仍是实际挂载点，可能位于发现根之外，
与内部的 `root_path` 不混用。选择额外绑定 `discovery_root` 及发现根
`device/inode/ctime`，一并进入内容摘要。

执行器必须在生命周期/设置协调锁内读取当前设置并调用
`revalidate_within(current_root, selection, exclusions)`，不能改用底层
不限定管理面范围的 `revalidate`。更改配置发现根、替换发现根或短期目录
变化会使旧预览失效，需要重新检查。旧 `browse/inspect/revalidate` 仅保留
给已明确界定范围的内部调用和测试，不能作为公开管理接口。

已开始激活的恢复仍根据原持久意图、原配置与核心 disk seal 完成，不以
后来修改的发现根、ctime 或重启后的 mount ID 重新选择目录。
