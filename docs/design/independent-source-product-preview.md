# 独立磁盘来源图的管理面只读预览

本次把已有独立盘 held-directory reader 接到实际管理 API 和磁盘向导。独立格式不会按旧 managed-directory 格式重新解释；候选仍保持“不支持直接加入”，用户可明确点击“只读检查独立盘来源”。检查不是自动发现、当前池接纳、降级、目录重建或迁入执行。

## 实际调用链

`foreign/independent.rs` → `POST /api/admin/storage/independent/preview` → `ManagementBackend::setup_operation` → `independent_disk_preview` → `reopen_source` → `PrivateDirectory` → `IndependentSourceReader` → 原完整元数据 verifier → `SourceTopology` → `source_topology_display` → 类型化显示契约。

请求仅包含原发现得到的 `DirectorySource`。认证、CSRF/origin 和有界阻塞任务复用已有管理入口；目录生命周期 gate 在整个同步检查中持有，不允许与当前池选择切换并行。SYSTEM、管理目录、当前成员和停止目录仍由服务端排除。检查前后重新核验发现父目录和直接孩子的物理观察，读取只来自原持有 FD，不从第二次打开替换来源。结果（包括失败）返回前再次验证当前登录会话，撤销优先。

旧 reader harness 把 disk_authority.rs 通过根级绝对 `#[path]` 外接，导致子模块落在错误目录。本次窄验证将测试 crate 根放在相应 src 目录，正常声明 `pub mod disk_authority;`，不为测试改写生产模块接线，也不把缓存 Engine 当成本次完整 Engine/server 构建。

## 输出与交互

完整图保留来源 pool/disk/slot/binding generation、incarnation、角色、所有节点 pin 和两端准确 publication。关系方向为来源 → 消费盘。不同 publication 引用不能归并成“最近一次”；节点的历史前驱链不冒充来源边。

契约解码检查节点身份唯一、pin 有序唯一、root 的 checkpoint/sequence 一致、端点身份与 pin 绑定、严格逐层关系、每个引用从选中根可达以及非一级 publication 的上游存在。它只证明显示结构，不产生核心能力、当前物理对应关系或任何写权。缺失正文不影响元数据图，也不输出正文可用性。

Web 先处理 401，再判断会话 epoch、请求 generation 与原 `DirectorySource`。刷新、返回、换盘、切到旧格式预览、关闭或会话失效复用 foreign.invalidate，清除结果并取消浏览器读取。服务端已开始的同步 I/O 仍由既有任务槽持有，不因浏览器断开释放；本次不声称中止底层设备调用。

## 成本与未完成项

复用原单盘4096对象/16MiB闭包预算；额外图投影为 O((V+E) log(V+E))，最多4096节点、4096关系、10000节点pin引用、1MiB完整响应。超预算失败，不截断成完整结果。无数据盘写入、fsync、writer lock、cache 或正文扫描。服务端当前管理分派仍经既有 Value 回应与最终HTTP编码，不宣称全链路仅序列化一次。

候选对应观察（零/一个仍不完整，多个匹配仍歧义）、跨请求捕获选择、来源映射交互、注册/角色转换/迁入执行、自动热插拔、真实浏览器和物理盘验收仍是后续工作。已有 candidate/session/observation 切片随源码保留，但没有作为此路由的匹配结果冒充完成。

实际编译、测试、未执行项以本次交付证据为准；设计文档、测试源码、格式检查均不能代替完整应用构建或浏览器验收。

## 验证入口与准确范围

正式 TCP 集成目标为 `cargo test -p storage-server --test independent_preview`，测试实现保留在根 `test/server/integration/independent_preview.rs`。它通过公开 HTTP 完成首次建号、登录、开发池激活和来源发现，再执行本接口，使用真实 ManagementBackend、独立盘文件与 verifier，没有模拟后端。

对齐 main `1add67b0` 后，完整 Contract、Engine、Server、SMB 和 Client 源码已在复用固定基础依赖的窄构建图中代码生成，Engine 按 Server 测试 manifest 启用 fault-injection。3项 TCP 集成和42项独立来源契约通过；Web 生产/测试及 Server 全部单元测试源码的 metadata 检查通过。Server 与新增 TCP 目标的 Clippy 通过。以上不代表完整 Cargo 依赖重建、release 配置或 WASM 构建通过。

全 Server libtest 代码生成受到3GiB地址空间上限限制，分配失败后停止；没有运行其中的新私有单元测试或旧路由矩阵，不能把 metadata 检查算作运行通过。TCP 目标独立覆盖真实认证、Cookie CSRF、Origin、会话注销、查询/未知字段/重复键/请求体上限、替换目录、受保护及越界目录、损坏拒绝，以及缺失正文时源盘和已登记盘字节不变。真实浏览器、取消/换盘交互和物理盘验收仍未运行。
