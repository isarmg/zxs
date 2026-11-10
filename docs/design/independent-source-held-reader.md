# 独立来源只读 held-directory 桥接

本切片组合 b120 基线、已独立验证的 lifecycle18 与 contract35，增加真实持有目录到 verifier 再到显示 JSON 的单向接线。原 reader 切片本身不是完整外来高层磁盘功能；其后续只读 HTTP/Web 接线见[产品只读预览](independent-source-product-preview.md)。自动发现、热插拔和登记入口仍未完成。

## 输入与权限

`IndependentSourceReader::from_directory` 消耗调用方提供的 `PrivateDirectory`。调用方负责认证、发现范围、挂载/排除策略和选择许可；这个 reader 不把“可以打开路径”升级成授权。生产路径不从显示 JSON 还原磁盘身份。

根和四个现有子目录均从同一持有 FD 打开，使用 `SameMount` 和拒绝符号链接的相对解析。诊断路径仍须与持有根相同；在开始与原 verifier 的结束边界检查命名身份。持有对象被移走、换成同身份克隆、链接或换掉子目录均不能自动采用替代对象。路径复核是历史观察条件，不是持续在线证明。

读取 header、初始化完成记录和子目录不会创建或修复内容。构造 reader 只准入格式和读取资源；`source_topology` 和 `observe_held` 才调用原完整元数据 verifier。没有读正文、容量、cache 或取得 writer lock。该类型无初始化、写入、正文读取、登记或转换到 `IndependentDisk` 的公共 API。

## 生命周期和有限候选

`SourceCandidateRequest::observe_held` 消耗一次性请求和一个显式目录列表。在读取任何 header 前检查128项上限。打开失败保留原输入索引和 `OpenDirectory` 阶段；后续使用原 `Capture`、`RecheckDirectory` 和物理关系错误，不把失败转成空列表或不匹配。

其余批次检查仍由原真实 verifier 负责：完整来源身份、全部 publication pin、自有链、每个输入行、别名/独立克隆、重叠、变化及10,000条自有链总预算。同一物理目录的重复输入保留多行但只计一次；独立克隆有两个匹配仍为 Ambiguous。零或一个匹配不产生完整性、唯一性或不存在结论。

响应只能经原 `SourceCandidateSession::receive` 交给当前请求拥有者。换请求、换节点、取消或换拥有者使旧成功/失败不可接受。取消接收不停止已开始的文件系统 I/O，上层仍须负责有界工作队列、并发背压、任务回收和事件失效。

## 只读传输出口

`encode_source_observation` 只接受真实 `SourceCandidateBatch`，显式单向转换到 contract35，再使用有界 writer 生成最多1MiB的成功 body。它逐项保留身份、root checkpoint、全部 pin、状态和计数；不序列化 dev/inode 或进程内 Arc 身份。Pin 复制在10,000个显示引用预算内，不能先无界展开再检查。

这个出口不是完整来源图协议，只覆盖一个节点。原真实图仍保留所有多来源边及两端准确 publication；尚无完整图 wire、来源选择的跨请求捕获 ID 或服务器 session/scope 封装。调用者应从当前已接收 session view 取得批次；不能直接把历史 batch 当当前页面响应。

该候选观察编码出口没有接入 HTTP 认证、401优先处理、CSRF/origin、请求关联和错误映射，也没有候选匹配 route。独立盘完整来源图的另一个只读接口已有产品接线，详见上述文档；真实浏览器取消仍需验收。新错误保留 Rust source 链，未来 HTTP 层必须保留输入位置/阶段和安全错误码，不能直接向客户端泄露内部路径。显示 JSON 解码仍只证明结构一致性，不产生任何核心证明或写权。

## 成本与验证边界

每次持有目录打开增加有界 FD 和 header/完成记录读取，不增加 fsync 或持久化。候选仍复用原 metadata、链和物理关系成本；最大128项，每项持有原五个目录资源。批次是有限历史观察，不是全局原子发现。编码为有界字段复制及 JSON 写入，最多10,000个显示 pin 引用及1MiB输出。

新增根测试覆盖真实临时独立盘、持有根替换/误名/子目录链接、读失败输入位置、损坏元数据、先行预算拒绝、空/单/别名/克隆/身份不匹配/缺 pin 状态、原请求失效、缺失正文和真实三级多来源多 pin 投影。额外两个测试检查输出字节边界。具体通过、失败、未运行以本切片收据为准，不把旧105或35项重复累加成新用例，也不把窄源码 harness 当完整当前 Engine/server 编译。
