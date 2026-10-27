# 磁盘列表：类型化读取边界

本轮仅迁移 `GET /api/file-manager/disks/{slot}/entries`；不改变写入、备份策略、权限、持久化格式或浏览器功能。

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
