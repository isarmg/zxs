# 20 工程规范：结构、实现与变更验证

[文档目录](README.md) · [上一章：容量、配额与性能预算](19-capacity-and-performance.md) · [下一章：配置规范与默认值](21-configuration.md)

<a id="s20"></a>

本章是目录、来源治理与工程变更流程入口；完整 Rust 编码与优化规则由根 [禁止防御性编程.md](../禁止防御性编程.md) 统一维护，[AGENTS.md](../AGENTS.md) 仅作摘要入口。目录和依赖段落描述当前结构，实现与评审条款适用于整个代码库，包括现有实现和新增变更。规范要求不表示存量代码已全部达标，也不表示每条规则已有自动检查。产品行为以对应领域规范为准，实际验证范围以绑定提交与执行环境的证据为准。命名细则见 [NAMING](NAMING.md)，测试布局见[测试分离规范](architecture/test-layout.md)，自动化覆盖见 [CI](ci.md) 和[质量门禁](quality.md)。

## 本章目录

- [20.1 Workspace 与目录](#s20-01)
- [20.2 命名与依赖规则](#s20-02)
- [20.3 离线工具](#s20-03)
- [20.4 构建与工程验证](#s20-04)
- [20.5 模块与接口设计](#s20-05)
- [20.6 错误与可观测性](#s20-06)
- [20.7 并发、生命周期与 unsafe](#s20-07)
- [20.8 测试、文档与变更评审](#s20-08)

<a id="s20-01"></a>

## 20.1 Workspace 与目录

```text
zxs/
├── Cargo.toml / Cargo.lock       # 唯一活动工作区与统一锁定解析
├── rust-toolchain.toml
├── src/
│   ├── auth/                    # 可移植认证算法
│   ├── contract/                # 业务契约、ID、严格 JSON 和校验
│   ├── naming/                  # 命名规则、固定映射和合并后的 Unicode 规范化
│   ├── platform/                # Linux 文件安全、SQLite 和凭据基础设施
│   ├── engine/                  # 存储、历史、授权、发布及恢复
│   ├── server/                  # zxs 服务入口、HTTP、认证、流式内容和本机 SMB RPC
│   ├── client/                  # 运维 HTTP 会话、传输及完整性验证
│   ├── rescue/                  # zxs-rescue 本机维护与离线恢复入口
│   ├── web/                     # 浏览器 WASM 产品界面、声明式样式与字体
│   ├── interface/               # 生命周期、主题、组件与合并后的 CSR 原语
│   ├── macros/                  # 界面过程宏编译目标
│   ├── smb/                     # Samba 协议桥接和 SCM_RIGHTS
│   ├── smb-vfs/                 # Samba SDK 对接和 Rust C ABI
│   └── foundation/              # 同仓维护的 Rust 基础实现
├── tools/                       # 根工作区中的构建、来源和验收组件
├── docs/                        # 所有活动文档和原始上游说明
│   ├── components/              # 自有组件说明
│   ├── tools/                   # 工具说明
│   ├── validation/              # 验收说明与证据索引
│   └── upstream/                # 原始上游文档，逐文件哈希保留
├── compliance/                  # 所有许可、版权、来源和迁移收据
│   ├── licenses/                # 第一方条款、逐来源上游原文和补充通知
│   ├── provenance/              # 原始身份、清单、哈希和目录合并映射
│   ├── NOTICE.md                # 统一第三方通知索引
│   └── rust-sources.json        # 活动源码来源与完整性清单
├── deploy/                      # 部署配置及安装脚本
├── test/                        # 按组件组织的单元/集成测试、夹具与证据检查
├── build/web/                   # 生成的正式 Web 资源
├── target/                      # 本机编译缓存与局部 SDK
└── runtime/                     # 本机运行池、预览和回滚材料
```

当前目录和集中维护约定见[单仓库结构](monorepo.md)。自有应用和工具属于根工作区；基础实现作为本地路径依赖接入，只有根 `Cargo.lock` 参与解析。`src/`、`tools/` 和 `test/` 中没有独立子工作区、活动子锁文件或嵌套 Git 仓库。来源身份和原始材料由 `compliance/provenance/` 统一保管。

构建按包和目标选择：默认成员为 Linux 服务及运维工具，Web 使用 `wasm32-unknown-unknown`，VFS 显式使用 Samba SDK。所有路径依赖均位于本仓库，禁止依赖其他项目绝对路径。

### 源码与本机生成物

`.cargo/config.toml` 默认输出到根 `target/native/`，Web 与工具使用根 `target/` 下各自缓存。正式 Web 资源由统一构建器写入 `build/web/`。各类缓存、运行池和生成证据按根 `.gitignore` 的限定路径排除。

| 纳入版本管理 | 忽略的本机内容 |
| --- | --- |
| 根工作区清单、根 `Cargo.lock` 和全部本地 Rust 源码 | `target/`、`build/` 和构建临时目录 |
| `src/web`、`src/interface`、字体、schema 和固定向量 | 根 `runtime/`、`zxs-data/`、本地缓存及待提交数据 |
| `docs` 中的全部说明、固定图示和验收索引 | 验收结果 JSON、截图、日志、收据和浏览器报告 |
| `compliance` 中的许可原文、来源身份和历史原文 | 凭据、私人环境覆盖和个人工具状态 |

验收说明统一在 `docs/validation/`，引用的原始检查代码与本地产生的证据位于 `test/evidence/`。固定输入样例与所属测试一起放在根 `test/` 的组件子目录；不能按文件后缀全局忽略依赖源材料和固定样例。

文档链接检查区分活动规范与原始上游归档。活动规范、schema、固定样例和检查脚本的链接必须存在，路径越界和符号链接越界也必须拒绝。文档允许引用按规则忽略的本地生成验证结果：存在时核对目标，缺失时单独统计，不把源码副本中缺失的生成证据误记为业务验收通过。`docs/upstream/` 保持原始说明字节，由来源映射的哈希校验，不把其中历史链接当作当前手册链接。

认证算法在独立可移植 `authentication` 中只有一个实现；服务端与救援工具直接依赖 `authentication`，服务端与平台均不保留旧认证路径兼容导出，业务会话持久化仍属于服务端。

核心按责任拆分：business/repository 管仓库与共享绑定，workspace 管完整上传和提交，history 管文件历史和关系，namespace 管路径，file 管共享文件分支与备注，directory 管稳定目录分支、不可变子树、选择及默认视图。container_storage 管普通正文唯一路径、真实容器改名、继承缓存与不支持布局的拒绝，拒绝时保留原字节和目录，不执行旧 stub 清理或迁移；边界见其[README](components/engine/src/container_storage.md)。engine/{publication,recovery,reads,index,workspace_storage} 管发布、恢复、读取、索引和工作区；journal 管状态分页及证明，metadata_budget 管决定前限额，maintenance 管计划、清理与当前支持范围内的历史树拆分（含目录组合、文件节点或独立文件分支时拒绝），不承担旧版本兼容；history_package 管历史包证明、对象图 I/O、导出与恢复导入，history_package/manifest 管历史域 State 投影和纯逻辑闭包校验，history_package/graph 管分页编码和图解析；repair/degraded_recovery 区分可修复与永久损失。入口复用核心规则，不直接操作业务盘。

文件节点的统一解析与继承标签由 `business/file_nodes` 管理；`file_tags` 新增标签子节点，`file_merges` 保存采用结果和归属，`file_publication` 按精确引用统一推进目录组成。旧 `history` 中的 `MergeLink` 仍只记关系线；新 `FileBranchMerge` 会推进归属分支与参与目录引用，两者不是同一操作。节点正文继续通过真实 `version_id` 读取，语义边界见[合并、标签与备份规则](design/file-merge-tags-backup.md)。

服务端 `file_manager.rs` 定义以磁盘、相对路径和稳定文件身份为入口的路由，`engine_backend/file_manager.rs` 将其映射到核心业务约束、命名空间和文件分支操作，不把磁盘当作分支。`telemetry/mod.rs` 保留最小 CPU/内存采样对象，磁盘容量由核心针对已登记盘根读取真实文件系统统计；接口只提供管理员基础状态，见 [采样边界](components/server/src/telemetry.md)。Web 的 `monitor.rs` 只展示这些真实样本，`history.rs` 展示已提交文件操作；两者不混用监测数据与文件历史。

`engine_backend/file_manager/backup_namespace.rs` 单独投影备份盘的用户逻辑路径，先过滤历史域授权，再检查路径冲突并固定该槽可用版本。它不伪造主路径上下文或提供备份写入入口；磁盘可见性按实际版本所属历史与路由判断，不能只按仓库默认一级盘推断。

Web 的 `files/{model,view,detail,inline,branch,version_tree}.rs` 分别维护浏览状态、列表、详情、原位输入及版本树；`files/actions/{browse,mutation,operation}` 分开处理读取、事务和回执。公共 `geometry.css` 的 stack/flow/row/frame/input 角色统一尺寸、内缩、边框和 G4；stack/flow 默认不增加外部间距，仅在相邻可见表面直接接触时用 surface 边界角色补距，已有文字或标签行隔开时不重复添加。业务样式只定义领域布局。详情通过顶栏菜单访问，返回文件列表时保留目录、搜索条件与选择；列表和详情共用原位备注输入，不提供正文预览。分支名称在原按钮位置输入，Enter 提交，空白、失焦或 Escape 取消，不再使用分支弹窗；前后端复用 `name-policy` 验证名称。Web 不含上传动作或正文传输任务页；保留的 HTTP 内容接口与客户端库不构成 Web 功能。公共 `Select` 管理键盘、焦点及原设计弹层；公共与业务文案均通过统一语言接口选择，CSS、令牌和图标仍为声明式资源。

<a id="s20-02"></a>

## 20.2 命名与依赖规则

### 项目命名

| 对象 | 规则 | 示例 |
| --- | --- | --- |
| 项目显示名 | `ZXS` | `ZXS 文件存储` |
| Cargo 包名 | 按职责命名，kebab-case，不加项目名前缀 | `storage-contract`、`web-interface` |
| 第一方组件目录 | 按职责组织，不重复项目前缀 | `src/contract`、`src/interface` |
| Rust crate 路径 | 对应职责包名的 snake_case | `storage_engine::Engine` |
| Rust 文件、模块、方法、字段 | snake_case，明确业务词 | `request_digest`、`commit_workspace` |
| Rust 类型与 trait | UpperCamelCase，在模块中表达职责 | `Workspace`、`ReadLease`、`CommitRequest` |
| 常量与外部错误码 | SCREAMING_SNAKE_CASE | `MAX_FRAME`、`CONTENT_PURGED` |
| 环境变量 | `ZXS_` 加职责 | `ZXS_WEB_TARGET_DIR` |
| DOM 标记、应用缓存键 | `data-zxs-` / `zxs-` | `data-zxs-runtime`、`data-zxs-theme`、`zxs-language` |
| 工具二进制 | 按职责命名；产品入口保留产品名称 | `web-build`、`contract-check`、`zxs` |
| 持久化 ID | 语义领域前缀加 32 位小写十六进制 | `ver_…`、`commit_…`、`op_…` |
| 哈希用途域 | `zxs:` 加用途 | `zxs:request\0` |

源码目录、包和模块按职责命名，不机械添加项目名前缀；第三方来源标识、产品入口和公开协议标记遵守各自约定。业务字段和持久化 ID 遵守公开契约，不因展示名称变更而更换语义。持久化记录和分段清单使用固定语义类型标识，不嵌入项目发布版本号；未支持的布局直接拒绝，不保留旧格式迁移分支。

复制并维护的框架实现属于本项目源码，遵守上述命名和独立目录规范。外部 Cargo 包、第三方仓库、SDK 函数、系统 ABI 符号和协议字段保留真实名称；不得通过文本替换破坏链接或伪称另一个依赖。第三方来源、版权和许可通知集中在 `compliance/`，原始说明集中在 `docs/upstream/`；来源、包身份和源码版权头保留真实上游名称。

### 依赖方向

```text
Web 界面 → web-interface（共享组件、生命周期和直接合入的 CSR 原语）
界面编译 → view-macros（过程宏）
服务端认证 / 离线救援 → authentication（无 Linux 或数据库依赖）
Web → HTTPS 业务 API
本机 zxs → storage-server 服务启动
Web 管理界面 → HTTPS 管理 API（初始化、账户、磁盘和维护）
本机 zxs-rescue → 未初始化配置草稿 / 系统与历史恢复 / 离线检查与导出
保留的 storage-client 库 → HTTPS 业务 API + storage-contract（不再由 zxs 暴露）
HTTP / 本机 SMB RPC → storage-engine
Samba VFS → smb-protocol → 受保护 Unix socket
storage-engine → storage-contract + name-policy + platform-io
platform-io → 文件系统 / SQLite / 凭据算法依赖
```

上图区分运行调用与代码依赖，不能把 HTTP、Unix socket 调用理解为 Cargo 边。第一方包间允许的直接依赖由[架构策略](../.github/architecture-policy.json)明确登记；当前边界如下（不列 Foundation 依赖）：

| 包或目录 | 运行/构建期第一方直接依赖 | 仅开发依赖 |
| --- | --- | --- |
| authentication、name-policy、platform-io | 无 | 无 |
| storage-contract | name-policy | 无 |
| storage-client、smb-protocol | storage-contract | 无 |
| storage-engine | storage-contract、name-policy、platform-io | 无 |
| storage-server | authentication、storage-engine、platform-io、storage-contract、name-policy、smb-protocol | storage-client（真实 HTTP 发布回归） |
| smb-vfs | smb-protocol | 无 |
| web-app | web-interface、storage-contract、name-policy | 无 |
| web-interface | view-macros | 无 |
| view-macros | 无 | web-interface |
| storage-rescue | authentication、platform-io、storage-engine | storage-server |
| contract-check | storage-contract | 无 |
| perf-check | storage-engine、platform-io | 无 |
| browser-smoke | storage-engine、storage-server | 无 |
| unicode-build、release-tools、web-build、web-bindings | 无 | 无 |

此表解释职责，机器策略登记准确边集合；增加包或依赖时同时评审二者，不能通过随手放宽策略使检查自动认可设计。当前第一方构建依赖没有第一方包边；过程宏实际使用编译目标的性质与 manifest 依赖表类别分别判断。根清单保持虚拟工作区，不添加根 `[package]`；成员使用显式路径，不使用 glob，以便审阅库存变化。非 Foundation 的仓库内路径包也必须显式登记为工作区成员与策略所有者，防止 Cargo 自动成员绕过清单。

[架构检查器](../.github/scripts/check-architecture.py)以 TOML 和真实路径解析包身份、依赖别名、workspace 继承、可选与目标条件依赖，区分 runtime/build/dev，检查运行与构建依赖无环；显式允许的开发期反向边不视作生产环。它不按当前主机 cfg 隐藏依赖，也不把 Foundation 内部一并当作第一方分层。Foundation 的布局、来源完整性和特性边由[来源治理工具](../tools/rust-sources/manage.py)校验；[消费者索引生成器](../tools/rust-sources/architecture.py)只生成并核对声明依赖与保守特性闭包的文档，不是 Foundation 分层规则检查器。这些检查都不证明模块内业务调用、授权与锁语义正确。

`storage-contract` 不依赖操作系统或 UI；`name-policy` 将 Unicode 规范化算法与项目映射放在同一编译组件，使用固定 Unicode 数据，不跟随编译器 Unicode 版本漂移。SQLite 为可删除后重建的索引，全部配置成员日志和不可变对象承担恢复依据。复制必须显式读写完整字节，不使用硬链接、reflink 或去重冒充独立副本。

真实文件容器的目录名、文件备注和版本备注由 State 恢复，`container.json` 只是派生缓存；不可变版本描述、内容散列和发布证明留在独立证明目录。`date-time` 负责 UTC 时间格式，`rustix` 负责已有安全文件系统原语。当前格式要求显式版本字段与分片对象布局，不承诺旧格式读取或自动迁移；各字段、暂存正文及恢复路径的准确边界见[第 8.2 节](08-full-storage-and-disk-format.md#s08-02)。

自研业务、界面、工具和桥接逻辑采用 Rust。HTML、CSS、TOML、JSON、Unicode 表和其他声明式资源按其用途维护；WASM 生成的 JavaScript 绑定及官方平台依赖可以包含其他语言。自写业务不得迁入这些外部桥接文件。Python、Bash 和 Node 脚本可用于来源治理、构建编排、测试驱动及浏览器验收；它们不是第二套产品业务实现，不承接核心授权、事务或持久化规则。具体工具入口见[工具目录](tools/README.md)。

API 与 SMB 只映射认证主体、输入、句柄和传输错误。入口认证由服务端统一执行；最终业务约束、文件占用、命名、备份路由和发布判定均在核心执行。普通应用不直接访问底层数据盘。

<a id="s20-03"></a>

## 20.3 离线工具

`zxs-rescue offline-check <disk>` 验证本盘版本、内容清单、发布回执和带状态页包含证明的内容墓碑；不修改盘数据。`zxs-rescue offline-export <disk> <version> <output>` 独立验证后输出完整文件，拒绝覆盖已有目标。导出不修改目标父目录权限，失败不把未完成复制发布成完整文件。

`zxs-rescue --root <new-root> restore-system --disks <disk1> …` 从固定配置的至少 Q = floor(N/2)+1 个一致、可信的原元数据成员配置恢复系统目录，随后重放日志、恢复凭据灾备快照及可发现的待提交工作区。该快照不构成管理面认证权威；守护进程启动和后台重试不会自动安装管理凭据。管理凭据恢复必须在另行准备的干净系统状态目录中，通过受当前本机 bootstrap 授权保护的 Web 恢复流程明确确认旧凭据风险；保留已恢复或部分恢复目录作为核验依据，不删除或覆盖现有权威。它不会重新初始化数据盘，也不绕过设备身份核验。N ≥ 1，单成员池须核验唯一原成员；多成员池不能按幸存数降低 Q。正常写入仍须全部配置成员及发布闭包，不因一份 PUBLICATION 就放行。所有新初始化盘均保存配置锚，缺锚旧盘不兼容且不自动迁移。

离线工具不依赖在线服务或原服务器 SQLite。实际只恢复本盘持有且证明、字节均有效的版本，不推断全局最新状态。格式、设备或日志冲突必须明确失败；检查命令不得隐式修复或清理。

<a id="s20-04"></a>

## 20.4 构建与工程验证

Rust 版本由根 `rust-toolchain.toml` 指定。所有组件、目标和工具共同使用根 `Cargo.lock`；在仓库根通过 `cargo ... -p <包名>` 选择编译目标。正式构建使用本仓库 Rust 源码与 `--locked --offline`，不在构建时下载 Rust 依赖；覆盖范围见[统一来源清单](THIRD_PARTY_RUST_SOURCES.md)，许可与来源规则见[集中合规说明](../compliance/README.md)。Linux 原生链接依赖和 Samba SDK 保持原有管理方式。

单元与集成测试按组件集中在根 `test/`，模块单元测试通过显式路径接入实现；跨入口浏览器、HTTP、CLI、故障及真实 SDK 检查按[验证说明](validation.md)执行。构建、模拟文件故障、真实设备断电、浏览器、SMB 互操作和发行产物是不同层级证据，禁止相互代替。

契约变更同时更新类型、schema、固定样例、Rust 校验器和入口映射；只保留一份规则实现。禁止未经证据将第 22 章全部用例一次标记为通过。


<a id="s20-05"></a>

## 20.5 模块与接口设计

以下是持续维护要求，不是对全部历史实现的达标声明。

- 以业务职责、状态所有者和变化原因划分模块。公开入口负责解析、认证上下文与协议映射，业务层执行对象归属、不变量与状态转换，平台层封装系统调用及持久化基础设施。不得在 HTTP、SMB、Web 或工具中复制核心判定。
- `mod.rs` / `lib.rs` 负责组合模块与必要导出；有独立不变量、资源生命周期或测试边界的流程应拆成具名模块。不以行数为唯一标准拆分，不创建没有职责的 `common`、`misc`、`utils` 聚合层，也不为目录整齐新增包。
- 默认使用私有或 `pub(crate)` 可见性。跨包 API 明确输入、输出、身份、前置条件、失败语义和资源归属；不为了测试方便扩大生产 API。适配器保留其边界，不把内部磁盘路径、数据库事务或共享锁暴露给 UI。
- 多个 ID、路径、代数或同类型参数使用具名输入结构，语义不同的身份不得以无名元组互换。HTTP 客户端不能自行提供服务端信任主体；服务端从认证上下文构造引擎输入。
- 时间统一基于 UTC，禁止按本机或浏览器时区隐式换算。服务端负责展示字段的一致 UTC 表示，界面直接展示；必要的时间间隔、图表坐标及单调时钟计算不引入额外时区层。具体单位和字段格式见 [API 时间约定](15-api-and-errors.md#s15-01)。
- 新增依赖先检查现有实现与真实消费者。Foundation 的职能目录是导航与治理分组，不是必须合并成的大包，也不改变实际包身份；当前库存以[来源清单](THIRD_PARTY_RUST_SOURCES.md)为准。合并须证明类型、特性、宏路径、消费者、平台和来源均得到保留或明确退役；不得把移动目录等同于减少依赖、内存或编译成本。
- 包退役、内部移动、跨包整合与升级分别遵守[来源身份规则](architecture/source-naming.md)及[Foundation 维护边界](architecture/foundation-groups.md#来源与维护边界)。不得为通过检查改写不可变来源收据。

<a id="s20-05-boundaries"></a>

### 禁止防御性编程

根目录 [禁止防御性编程.md](../禁止防御性编程.md) 是全仓完整 Rust 编码与优化规范，适用于存量与新增代码：在真实信任边界解析为受约束的类型，内部信任类型与调用契约；内部不变量失效快速暴露，外部失败按其来源处理。交付附边界说明。存量实现须逐项核对，规范落盘不表示全仓审查已经完成。

<a id="s20-06"></a>

## 20.6 错误与可观测性

- 业务失败使用明确的领域错误或既有契约错误码；协议层统一映射 HTTP / SMB 表达。错误码及可重试条件以[API 与错误](15-api-and-errors.md)为准，不从错误字符串反推业务分支。
- 保留错误来源及必要上下文，区分输入无效、无权限、冲突、暂时不可用、完整性损坏和永久损失。不能吞掉失败后返回成功，也不能把所有底层失败都变成“未找到”。
- 用户输入、磁盘内容、网络、设备与普通资源耗尽不应通过 `unwrap`、`expect` 或 `panic!` 处理。确为内部不变量或运行时初始化契约的 panic 必须说明触发条件；测试和确定常量构造与生产失败路径分别评审，不以全仓字符串数量代替判断。
- 操作结果不确定时先查询或验证，不盲目重放可能已发布的变更。清理失败不能遮蔽首要失败；恢复、重试和补偿都必须保留可追踪身份及明确边界。
- 日志服务于定位问题，不成为认证或恢复权威。不得记录密码、会话令牌、完整密钥或正文；路径、用户身份和设备信息仅记录诊断所需内容。计数、耗时和采样必须标明范围，不把采样结果描述成全量事实。

<a id="s20-07"></a>

## 20.7 并发、生命周期与 unsafe

- 对共享状态明确所有者、锁保护范围、锁顺序与发布点。避免在持有核心锁期间执行可分离的长时复制、散列或网络等待；若一致性要求必须持锁，应解释原因、上限和失败策略，不能为减锁削弱验证。
- 分离准备阶段只能持有具有明确生命期的快照、资源句柄及校验凭据；提交前重新核验基线、授权、代数和相关身份。准备成功不等于发布成功。当前工作区准备仍有准入围栏，发布保持串行，见[分离准备](architecture/detached-workspace-commits.md)与[写入准入](architecture/detached-write-admission.md)。
- 脱离核心所有者的写任务必须持有相同 OS 锁的重复句柄，直到准备结果提交、取消或销毁；互斥锁的释放不能提前释放跨进程写入排他性。
- 文件系统边界区分“安全父目录下叶节点不存在”与父目录异常、链接、特殊文件或 I/O 错误；不得用 exists() 将损坏权威材料解释为空状态。需要发布已验证内容时绑定持有的文件身份，而不是再次信任可替换路径。
- 任务须明确取消、超时、关闭与异常后的资源释放。禁止无所有者后台任务或静默丢任务；线程局部资源不得以全局函数指针伪装成跨线程执行器。`Send` / `Sync` 和共享引用不能只靠命名或注释推断。
- 浏览器事件监听、订阅、定时器与异步回调由组件或 owner 生命周期管理。卸载后释放资源，避免回调保留不必要的强引用或写回已销毁状态。响应式状态、DOM 渲染与业务组件分别维护，见[浏览器运行时](architecture/reactive-browser-runtime.md)。
- 优先使用安全 Rust；必要的 `unsafe`、FFI、原始指针和 `unsafe impl` 限定在最小边界，邻近说明安全依据：有效范围、对齐、初始化、别名、线程条件、资源释放及调用者责任。安全包装必须验证外部输入，不能把未验证条件藏在安全 API 后面。
- ABI / FFI 边界说明所有权转移、错误返回、panic 边界和 SDK 版本。实际 SDK 构建、平台测试与普通 Rust 模拟测试分别记录；生成绑定的局部 lint 例外不能推广成手写业务代码豁免。

<a id="s20-08"></a>

## 20.8 测试、文档与变更评审

### 优化取舍

优先修复可复现的日常使用错误、重复工作和有测量依据的性能瓶颈。评审先说明触发场景、影响范围、收益及新增复杂度；能复用现有原语或删除多余步骤时，不增加平行实现、通用框架或无消费者的配置层。

可维护性是优化的前提。局部收益不能以模糊状态所有权、增加隐式跨层依赖或复制业务规则为代价；评审须沿真实调用链核对职责和依赖方向，而不只比较改动行数或基准数字。收益不足以抵消接口、生命周期和验证负担时，保留较简单的实现。优先消除重复与含义不明的参数，不为统一外观合并语义不同的入口。

低概率且低影响的假设场景，不应引入大段专用防御代码；没有可达路径或收益证据时先保留调查结论，不为凑修改而实现。认证、数据完整性、内存安全与已承诺的恢复边界不因发生频率低而省略必要保护，但应选择最小、可验证的实现。

### 测试选择

测试位置、入口与来源规则只在[测试布局](architecture/test-layout.md)维护。新增测试必须接入实际 Cargo / Python / 浏览器入口，并核对运行数量和所需特性，不能以空测试成功替代验证。

- 纯规则：覆盖边界输入、非法输入及不变量；回归缺陷应先给出可复现用例。
- 状态变更：覆盖成功、授权失败、冲突、重复请求、取消、错误注入和恢复后的可观察状态；不能只断言函数未 panic。
- 并发与生命周期：覆盖关键交错、过期基线、关闭及资源释放。尽量使用可控同步而非任意睡眠；随机测试保留种子与复现方式。
- 跨层行为：以真实消费者检查接口、序列化、特性组合和平台边界。单元测试、模拟文件故障、浏览器、SDK、SMB 互操作、物理盘掉电分别提供证据。
- 性能：报告提交、环境、工作负载、数据规模、重复次数和基线；区分逻辑写次数、耗时、产物大小与内存。移动源码、减少包数或一次 tmpfs 测量都不能独立证明生产性能改善。

### 文档与契约同步

维护文档以中文为主，代码、标识符、协议术语及必要原文引用保留原样；许可证、上游原文、来源收据和历史验收证据不得为了统一语言而改写。生成文档通过其生成器更新，不手工维护第二份事实表。

变更应遵循“一个主题一个规范入口”：产品行为在编号领域规范，工程目录与流程在本章，完整编码与优化规则在根规范，命名细则在 `NAMING.md`，架构专题解释实现与限制，执行时点证据保存在 CI 产物或独立验收归档，不新增项目进展报告。不得在 README 中另立与主规范冲突的规则；发现代码与规范不同，明确缺口并同步修复，不能悄悄将规范降低为现状。

API / 格式变更同时审查 Rust 类型、schema、OpenAPI、固定向量、校验器、HTTP/SMB 映射、调用方和文档；按实际影响更新，不要求无关文件机械变更。重大权威来源、事务协议、持久化格式或平台边界变更应在 [ADR](adr/README.md)记录选择、替代方案和影响，再链接主规范。开发期不承诺旧版本兼容，但不得把这一政策当作删除用户数据、绕过恢复验证或重置运行池的授权。

### 合入前检查单

1. 写明目标、不变量、影响模块及明确不包含的能力；核对系统盘管理独立于数据盘故障、单数据盘合法、Web/SMB 产品边界等相关约束。
2. 复核依赖方向、公开 API、授权与身份来源、错误语义、并发资源和必要的 unsafe 安全依据。
3. 同步当前规范、代码、契约和测试；验证结果必须对应实际执行的构建与环境。涉及来源材料时走现有来源治理，不手工绕过哈希与身份校验。
4. 运行受影响的快速测试与对应门禁，必要时运行完整 CI。命令和环境要求以 [CI](ci.md)为准，检查实现以仓库脚本为准。新增规则若尚无自动检查，应明确保留人工评审责任。
5. 记录具体命令、提交、退出状态、测试数量、环境与结果位置；将通过、失败、未执行、环境受限和待硬件验收分列。报告不能只写“测试通过”。

当前第一方包边界（`python3 .github/scripts/check-architecture.py`，同时接入 `static`）、格式、Clippy、编译诊断、契约与来源检查是自动化门禁；职责划分、错误上下文、锁顺序、生命周期和 unsafe 论证仍需要代码评审及有针对性的测试。本章不宣称这些语义要求已经被静态工具完整证明。

---

[文档目录](README.md) · [上一章：容量、配额与性能预算](19-capacity-and-performance.md) · [下一章：配置规范与默认值](21-configuration.md)
