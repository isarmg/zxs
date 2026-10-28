# 磁盘列表：类型化读取边界

服务端类型化读取覆盖 `GET /api/file-manager/disks/{slot}/entries`；后续 Web 准入与消费者迁移见文末。两者不改变写入、备份策略、权限、持久化格式或浏览器功能。

## 实际调用与表示

- Web `files/actions/browse.rs` 提交路径、搜索、页大小、前后游标和独立选中项；`files/batch/actions.rs` 用 `limit=1` 查询目标目录上下文。Rust client 没有该路径的专用方法，保留其通用 HTTP 请求能力，不发明兼容入口。
- HTTP `ApiQuery<BTreeMap<String, String>>` 保留重复键拒绝；`ReadListing::parse` 只接受当前六个 query 字段。路径缺省／空串为根；lookup 拼写保留 NFD，匹配用已解析键。页大小缺省 200、范围 1–500，搜索上限 1024 UTF-8 字节，游标上限 32768 字节；选中项仍要求完整身份对和直接子项关系。
- `Backend::listing` 返回拥有的 `ListingSnapshot`，主盘和备份盘实际 builder 都不构造条目 `Value`。HTTP 层在引擎锁释放后转换 wire DTO；计数、大小和分支序号仍为十进制文本。JSON 对象键顺序不是协议，条目顺序、目录分支顺序、字段缺省与 null 区分保持原定义。
- 虚拟目录只有原七字段；当前目录保留仓库位置、空备注等字段但没有文件正文字段；历史备份文件保留 `repository_path:null`、`selected_branch_id:null`、`historical_only:true`。备份文件仍覆盖对外 `version_id` 为备份版本，但不错误重写继承来源或 `own_version_id`。序列化不生成重复 `version_id` 键。

## 信任边界与动态判定

HTTP 中间件先认证，阻塞槽内解析静态 query，随后 AuthorizedBackend 与 EngineBackend 分别核对当前认证与引擎准入。静态无效字段在排队后当前身份检查之前报错，与本项目其他类型化路由相同；合法请求必须在触及资源前完成当前身份检查。没有用游标缓存权限。阻塞读取持有准入许可与认证所有者直到完成；取消 HTTP future 不提前释放后台工作依赖的资源。

解码的游标不是有效快照证明。引擎当前锁内仍绑定完整 config 与 business generation，以及 slot、路径匹配键、搜索、limit、排序名称和两个 Unicode 版本。先判请求签名，再判陈旧快照 `LISTING_CHANGED`，最后拒绝被篡改的排序键；与原有错误优先级一致。生成游标仍有编码字节上限。

主盘根可能只有虚拟仓库或尚未建根；根路径特定 `FORBIDDEN`／`NOT_FOUND` 合法展示分支保留。备份只读、全命名空间冲突检查、当前路径／历史 creation-name 回退和版本路由可用性保持。目录分支匹配直接使用已解析相对路径键与已解析仓库，不再次校验同一静态路径。页候选索引来自同一锁内的当前 namespace，越界是内部不变量 bug，而不是外部缺失。

## 成本与验证范围

仍扫描主盘平坦 namespace，保留至多 `limit + 1` 轻量候选和独立选中候选，再只拥有被选中的投影。备份仍构造受 `max_entries` 限制的逻辑节点图以检查全局冲突。目录分支数不受条目分页限制。未增加跨请求缓存、Arc、锁或 fsync；未宣称测得性能提升。

新增／迁移测试覆盖 wire 全字段、pending/backup version 覆盖、无重复键、大小文本、query 边界、真实 HTTP 成功与拒绝、前后翻页、签名与篡改、虚拟根、历史备份和快照脱离引擎的拥有性。所有 Backend 真实代理、HTTP panic 与取消槽位夹具及撤权矩阵均接线该方法；R20 基线的 70 请求增加到 71，保留 managed/embedded × logout/password 四场景及无凭据清单。

候选编写阶段按任务限制仅做源码与轻量静态核查；当时未执行 Cargo 编译／Rust 测试、真实浏览器、socket 或 Samba SDK。后续集成验证单独记录实际运行结果，不把源码接线或轻量核查当作运行通过。

候选阶段轻量核查命令（均通过）：定向 `rustfmt --edition 2024 --config skip_children=true --check`；`git diff --check`；`python3 .github/scripts/check-architecture.py`（20 owners / 30 edges）；官方 `manage.py refresh` 与 `verify`（224 active / 107 retired / 3 superseded）；`architecture.py refresh/verify`；`documentation.py refresh/verify`；`check-workflows.py`（5 workflows）。refresh 使用官方 metadata 流程，不构建 Rust 目标；不可变来源收据未手工编辑。

## Web：完整目录页准入与内部类型

Web 的 `files/page` 在已有有界 HTTP body／strict JSON 解码后一次性消费响应，形成字段私有的 `Page`、`Entry`、`DirectoryBranch` 和目录上下文。普通浏览请求的 200 条上限与批量目标目录查询的 `limit=1` 都传入同一入口；缺失必需字段、错误 null／标量类型、未知字段、非规范十进制数、跨盘／跨目录身份、重复逻辑路径、无效种类与文件内容状态、分页游标／计数矛盾均拒绝整页，不先安装部分条目或可写状态。错误保留 `InvalidDirectoryPage`、HTTP 200 与具名解码来源；路径与整数解析原因进入 source 链。

当前 wire 仍由服务端唯一负责，未改协议或新增兼容默认值。仅当前协议明确省略的 `backup:false` 被解释为主盘；显式 null 和缺字段不混用。虚拟目录七字段、带仓库位置的当前目录、历史备份的 null 主路径、待写入分支的空 source／null 可见版本、备份版本覆盖而继承归属不变均有独立准入分支。稳定文件身份可以出现在不同逻辑别名路径，不能误按 entry_id 全局去重。目录历史分支记录的路径也不被强行改为当前路径。

响应入口信任私有调用方的请求身份：slot 来自已准入磁盘，页大小只有 200／1，捕获的选择来自当前目录且导航会清除选择；不把这些内部契约再次当作无效服务端响应。普通导航只有根、已准入条目路径及其面包屑前缀，派生一次 `DirectoryKey` 后携带到响应匹配；批量目标输入仍在发起请求前按原命名规则解析为 `PortablePath`，将其缓存 key 与可信磁盘前缀组合，不在响应返回时重读或重验输入。空目标请求使用磁盘前缀本身，不再拼出无效尾斜线。响应自身的 slot／路径匹配、返回条数不超过请求上限、选择身份与直接子项关系及其余完整性检查保持。

`Model` 的一页条目、独立选择与目录分支不再保存 JSON；行、详情、编辑上下文、备注／分支请求和备份设置使用只读类型访问器。页面刷新仍使用会话／代数栅栏，旧请求不覆盖新选择，独立选中项仍可位于当前页之外；服务端确认选择消失才清除详情。历史 API 的独立响应模型未借此改写。批量操作通过结构转换接收目录分支摘要；其独立 summary／selection-facts 响应在自身入口准入，后续请求组装不重新检查已保证的 ID／树字段。后续提交仍由服务端重新授权与核对基线，UI 类型不是授权证明。

保留同一 `Arc<[Entry]>` 的页级共享、行稳定 key、服务端排序和现有控件生命周期；移除刷新中的重复 checked-entry 集合扫描。页与 scope 准入在既有响应字节上限内执行，页内身份检查为 O(P log P)，目录分支为 O(B log B)，未增加锁、I/O、fsync 或跨请求缓存；不宣称测得渲染或吞吐提升。未更改视觉、G4、浏览器能力或无障碍功能。

根 `test/web` 增补完整边界拒绝、pending／backup／虚拟目录、错误来源、选择身份与页面应用回归；原有行、详情、批量作用域、会话与编辑生命周期测试通过真实入口构建完整夹具，不再向内部类型塞入残缺 JSON。边界拒绝测试直接变造完整 wire 输入，不通过 fixture 补全掩盖错误。

本候选仅执行固定 1.99.0 工具链的逐文件 `rustfmt --edition 2024 --config skip_children=true`／`--check` 与 `git diff --check`。依任务限制，**未执行** Cargo、Rust 测试、构建、仓库治理脚本、浏览器验收或 CI；静态审阅与格式通过不代表编译或行为通过。
