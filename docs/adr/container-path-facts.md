# 文件容器路径事实与内容节点分离

状态：按用户已确认的“不保留一切兼容”和“只有内容变更创建节点”继续实施。
用户已明确获知旧池、旧备份整体不能由新格式打开，且原数据不删除、不自动修改。
本批是在该范围内更换路径事件模型，不新增迁移、旧节点解释或自动覆盖策略。
基线为 `84b574a4d8b12416f33f246d84162285cd4df487`。下文是本批实现合同与验收要求，
不是 GitHub 已通过、真实 Chrome 已通过或完整操作时间线已完成的声明。

## 决定

同一 FileId 的改名与移动改变位置，不改变正文身份，也不创建路径节点。
纯路径操作保留实际正文版本、内容图节点、父边与文件分支 HEAD；若同一工作区同时保存
新的正文，只有真正的新正文版本进入内容图，路径变化是同一操作中的独立事实。
目录范围操作只处理其包含的现有文件容器位置，不借此新增普通目录功能。

原路径节点不能被删除后假装数据完整，也不能被映射成某个正文版本。
新格式在 State、history、FILE 的真实准入拒绝这类旧节点；旧外壳在池启动、pending、
archive、snapshot/history 和 FILE 读取边界拒绝。旧格式数据原地保留，不能因为
新程序拒绝而重置池、清理目录、重算旧摘要或覆盖原收据。

删除、撤销删除与导入删除状态仍使用各自真实来源。Merge 等其他非正文元数据写入
尚不由本片全部收口；本片只证明实际改名、同身份移动和复制到达不再制造路径节点。
不能将这一切片表述为所有非内容操作已经完全退出内容图。

## 唯一事实来源

永久操作收据保存必需的 typed `path_changes` 数组，实际无路径变化时明确为空。
对具有该合同的操作种类，缺字段、错误字段类型、重复身份或不匹配的事实均拒绝，
不能用默认空数组掩盖旧收据或不完整来源。

每项事实具有独立 `FilePathEventId`（`fpath` 域，不能用作 `FileNodeId`），包含：

- 原 operation、file、entry、history、branch 身份；
- 实际变更前后 repository 与完整 PortablePath；
- 变更前后正文 VersionId、原发布序号、服务端真实时间与 Principal。

事实 ID 在写入时由操作身份、文件／条目身份和完整前后位置的规范编码确定。
同一操作中的同文件别名也必须可区分。禁止在查询时用页内位置、当前路径、当前时间
或排序序号制造事件身份。纯位置变更的前后 VersionId 相同；正文和位置一起改变时
绑定真实新旧版本，不能为了显示路径事实伪造新版本。

普通写、Current 实时写、序列化 candidate 冷回放均使用同一事实合同。
准入必须覆盖所有 namespace 早返回，精确比较实际前后位置及版本变化，确认没有
遗漏、额外事实或不属于该操作的事实。Current 的最终 Publication／D3、现场锁、
文件身份、授权、取消与原恢复材料校验不因改名节点退役而省略。

完整 Snapshot 准入与页面读取共用永久账本封套校验，绑定 operation 表键、发布序号、
主体和本地事实，并拒绝缺失的 Graph／Retained／移动复制保留封套。
本地事实及复制后的接收映射须引用存在且归属吻合的历史／分支／正文版本；
正文版本及借用证明不得晚于发布。
借用正文以永久 commit 的 pending 与当时条目配对证明，不要求已消费的当前 pending 仍在；
按需索引一次并在成员遍历中检查取消，不以今日路径或 HEAD 重建过去的来源。
查询另一个文件也不能绕过全局坏封套。该检查验证最终 State 中封套与身份的完整性，
不是从最终 State 重放全部过去命名空间；实际前后变更仍由提交及恢复的精确比较证明。

批次收据保存必需的原 typed namespace request，并与请求摘要绑定。
transfers 的成员数、动作、逐项 source／target 和真实前后位置必须完整吻合。
Move／Rename 缺失目标不能当复制跳过，清空 transfers 和 path_changes 也不是空证明。
新格式 transfer 的 file_id／version_id 必需且可空：目录为 null，文件为合法 ID，
不接受旧空字符串哨兵。位置证明采用成员索引，不对全局 occurrence 做二次交叉扫描。

纯改名保留文件分支不可变 HEAD，工作区不能再把 HEAD 旧路径当作唯一当前位置。
文件分支工作区捕获真实选中别名，显式上传路径选择一个 occurrence；stage、range、commit
与 SMB 后续选择复验条目／上下文／正文及祖先选择代数。不能任取首个别名或把同一工作区
改绑另一路径；创建后又发生改名时拒绝且保留暂存内容。完成与冷恢复只按已证明的原发布
刷新实际提交位置，不用后来的任意 context 重绑旧工作区。无正文变化的保存不因位置投影
制造提交，也不改写原 Current 历史快照。

## 复制与 FILE

新身份复制没有“此新文件的旧位置”，因此不是本地 PathChanged。
复制到达不创建路径节点，也不把来源操作时间替换成复制时间。

历史复制／保留移动在 SYSTEM 范围的 typed 保留封套中携带原路径事实及明确映射后的
目标 file/history/branch/版本关联。复制的复制保留扁平、真实来源证明；不能只保存
一个摘要却丢掉被承诺保留的事实，也不能把原事件重新编号成目标本地发布。
原来源身份、序号、时间和路径不作自动改写。当前 UI 不把来源事实混入目标本地
时间线；保留不等于已提供跨来源完整时间线。

FILE 记录只保存本文件归属、原事实身份／来源及映射版本的无路径投影，不保存外部
repository/path。它不能独自证明或重建外部命名空间位置。实际 FILE 读取仍检查
本地所有者、history、版本闭包和审计来源；新增事实行及其增量依赖须与生产编码同批。

同池完整历史复制与普通 namespace Copy／Separate 不同，后者只派生当前正文新容器。
跨池 foreign recipe 尚无完整新审计的目标映射，遇到非空路径事实或完整 Graph-copy audit
明确拒绝且保全原来源，不静默导入、丢弃事实或自动迁移。

## 只读协议与界面

真实详情继续使用 `/api/file-manager/files/{file}/history/lifecycle`，但事件采用
严格的 nested `detail`：Delete、Undo、Import 或 PathChanged。PathChanged 包含
`event_id` 和真实 `before`／`after`，界面称“路径变化”，不根据字符串猜测操作类型。
Delete 与 PathChanged 使用确实记录的时间；Undo／Import 仍为明确 `null`，不借用
删除时间、浏览器时钟或序号。旧 flat 事件响应不被新客户端接受。

分页锚点为 `(sequence, operation_id, source_id)`，其中 source 为真实 fdel/fpath。
游标固定为 `binding:operation:sequence:source` 四段，旧三段游标明确拒绝。
绑定仍包含 file、当前 root、解析后的上下文、请求位置、页大小和观察 State 序号。
同一操作可以有多个不同来源成员；同 `(operation, source)` 重复、逆序、cursor
成员重放、来源找不到、跨身份或缺失证明均失败，不能变成空成功末页。

响应最多 64 项、64 KiB。字节预算可产生短的非空续页，下一游标必须精确指向本页
最后真实成员；单个事件都超限时明确 ResourceLimit，不截断路径或字段。
取页前目标事件摘要最多 65,536 条，紧凑 JSON 数组编码累计最多 12 MiB，包含所有
Delete／Undo／Import／PathChanged。超限整次读取失败，不返回看似完整的部分页；
这约束目标摘要，不把整个 State、全部收据或进程内存声称为 12 MiB。
完整来源准入先于取页。每页扫描完整永久账本，并解码各复制／移动的必需保留封套，
再读取目标删除／恢复前缀，不仅是廉价的目标归属分类。逐收据及本地事实之间可以取消，
单封套 JSON 解码不能在内部打断；没有全局缓存。分页不意味着 O(page size) 或统一 CPU 上限。
浏览器使用 file、operation 和 source 的完整成员身份，不以 operation 单独作为卡片键。
关闭、换目标、注销与晚响应隔离、原错误呈现、读许可和授权复验全部保留。

## 格式门

| 材料 | 本批标记 |
| --- | --- |
| Genesis seal | `container-path-facts-managed-directory` |
| Generation seal | `container-path-facts-membership-generation` |
| Pending seal | `container-path-facts-membership-pending` |
| serving archive root/binding | `container-path-facts-frozen-control-metadata` |
| retained-exit archive root/binding | `container-path-facts-retained-exit-quorum-control-metadata` |
| preparation archive | `zxs-container-path-facts-backup-native-control` |
| snapshot/history root | `container-path-facts-radix-map-roots` |
| FILE rows | `file-local-container-path-facts` |

这些标记与内层严格事实准入同批发布，不仅替换字符串后声称闭合。
既有目录名、线程名、I/O 次序与备份算法不改。前批 container-tags 标记及更早标记
都不保留可执行兼容分支。旧 seal、混合成员、旧 archive 外壳、当前外壳加旧内层、
旧无 seal staging 的拒绝覆盖要保留；错误必须保留来源与清晰的不支持格式诊断。

沿用 [容器标签 ADR](container-tags.md) 已定位的入口逐项复核。
DiskSeal 只覆盖其实际消费者，不能冒称替代 history import、FILE rescue 或所有恢复门。
拒绝证明比较受控目录集合及文件原字节，不据此声称 mtime、权限或控制锁绝无变化。

## 验收与记录

本地只读写代码、git 整合和静态审阅，不运行编译、测试、formatter 或选择器。
GitHub 对最终准确 head 与比较基线做定向选测，并保留所有共享规则、每片预算、
原 900 秒备份事务和失败聚合；不把无回执、blocked 或只编译算作行为通过。

必须覆盖真实纯改名、同身份跨位置移动、save+rename、多别名批次、精确重试、
冷重开、Current/无 Current candidate、FILE 读回及历史复制来源保留；HTTP 验证
真实写入后分页、授权／根失效、错误和冷重开；Web native/WASM、稳定成员键、
同操作跨页、晚响应及严格响应拒绝均须实际执行。

旧 history 固定 golden 保留原字节。测试仅以明确根 discriminant 的字节替换构造
新格式 codec 期望，并用独立原始域 hash 对照原固定摘要，再核完整对象束；不是
新真实业务捕获，不重写旧证据或把本地生成内容称为 GitHub capture。
本批最终提交、工件、各失败与未执行项将在取得证据后记录，当前没有通过结论。
