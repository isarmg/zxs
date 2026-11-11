# ZXS 当前任务清单

- [ ] #1160 quality：按准确 CI rustfmt 日志修复原批次撤销相关 16 文件的 53 处格式差异，保留后继 WASM 默认展开修复；未本地编译、测试或运行 rustfmt，待准确提交 CI 验证。

## 当前 CI 与失败闭环

- [ ] 验证按实际变更选择的 GitHub 定向 CI，核对当前编译库存、非空选择、逐测试完成与未执行清单。完整 workspace、ignored、跨特性及其余恢复场景尚未验证；后续语义变化须审阅并扩展持久路径映射。

- [ ] 完成 D 审阅的真实浏览器验收：准确页面／节点／读取代数／会话绑定、旧行及旧续页回调隔离、分页范围完整、恢复后第一页重读与原未知结果围栏。相关原生定向测试已通过；超预算 D 的分页／紧凑恢复已实现，其剩余验证见删除与审计项。

- [ ] GitHub CI 验证合法同 scope alias 逐路径审计与全文件 D 生命周期，并验证四类软删除／恢复全局活动投影：原路径、不同主体、准确序号／节点、整批每文件一事件、精确重放与跨仓移动后冷开保留。

- [ ] 按每批实际变更只运行相关定向测试，核对准确 main 提交的选择范围、当前二进制库存与逐项完成证据；继续处理已知失败。未选择的 static、quality、contracts、security、workspace 分片、生产 WASM、一期 core、Samba SDK／真实 Samba、独立 ELF 与 Chrome 验收仍未完成，不以定向通过替代完整覆盖。
- [ ] 验证 13 片的当前产物／测试库存、分配并集与互斥、完整回执、11 个 I/O 门禁和三项显式 ignored 验收；验证参考配方不受分片环境污染、保存配方可达性及失败证据。完成各功能项仍待执行的原生、集成和浏览器测试。
- [ ] GitHub CI 验证新版 T02 计数棘轮：14 个第一方范围、24 项指标、新基线、接线与排除范围；增长和未更新基线的下降均拒绝。核对旧六目录口径的待验记录由新版覆盖。
- [ ] GitHub CI 验证新版 T13 磁盘满映射：`ErrorKind::StorageFull` 保持 `NO_SPACE` 与来源，`QuotaExceeded` 继续 `IO_ERROR`；errno 28 回归仅 Linux，ErrorKind 回归覆盖所有目标。
- [ ] GitHub CI 验证跨已提交目录的 `object.move` 恢复：双故障、原操作冷开重试、目标当前版本映射、两版本三盘独立容器、源 inode／正文与冻结视图、二次冷开无重复发布。
- [ ] GitHub CI 验证保留移动意图的 source 身份改动返回 `IdempotencyKeyReused`，首次捕获漂移返回 `WorkspaceChanged`，失败保留原意图与完整 State。
- [ ] 解决 SQLite 派生存储更新后的 checkpoint 失效及 workspace namespace／history 失败族；验证容器重命名的已持有版本序号、未知收据／主体表变化的完整历史重建、rename 后目录快照路径／物化序号、显式重建哈希与冷开一致性。
- [ ] GitHub CI 验证冻结目录预览夹具：合法同目录 rename、分页／只读／版本／游标、陈旧快照与撤权、稳定身份及全状态不变、结构 Delete 拒绝且无发布。
- [ ] GitHub CI 验证授权 backend 的 `ReviewedCreate` 准确 source 和完整参数／身份／结果／撤销矩阵；验证 HTTP 目录分支 panic 夹具实际进入 backend，保留 `INTERNAL_ERROR`、500、可重试与隐私断言。
- [ ] GitHub CI 验证首管理员 0700 私有目录夹具：准确重复创建、后续会话冲突、不同账户／密码拒绝及凭据原字节保留；验证 contract 严格可空／空变体解析和 auth 夹具。
- [ ] GitHub CI 验证 Backup serving 保留 Genesis discovery config，冷启动由已认证 native bootstrap extension 选择扇出，完整事务与精确重试保持 discovery 原字节。
- [ ] GitHub CI 验证离线激活精确重试的完成收据保留、回调与撤权失败行为。
- [ ] GitHub CI 验证 #1164 原批次撤销清单的 WASM HTML `open=""` 属性，保持默认展开与用户折叠。
- [ ] GitHub CI 验证登录前安装浏览观察器后的首次主盘浏览：磁盘身份、空根、重读、浏览无变更；验证当前四个管理入口、八项主导航、双语只读动作与反馈生命周期。
- [ ] 复验待验的格式／Clippy／类型及接线修复，覆盖 SQLite 新来源与特性图、安全公告、依赖／许可材料、CLI golden、私有目录夹具、独立 mount namespace、helper publication 故障与真实 SDK 编译诊断。

## 新版规范的底层重构

- [ ] 定向 CI 验证 T07 首批真实类型边界：Name／LookupName／SlotId 构造与原文本 serde、RepositoryName、Foreign 目标映射与槽位、历史保存名称、server listing 和源槽位搬移。名称错误顺序、碰撞 key、路径预算与原 wire 保持；manifest 准入与其余公开 validator 尚待逐域迁移。文本 parse_discard 模式亦匹配本批六处持有结果的 let 绑定，准确更新计数并保留规则，不以计数名称代替数据流审查。
- [ ] GitHub 定向 CI 验证 T08 HTTP 备注兼容切片：ApiJson<NoteBody> 与 Authenticated 接线、路径目标独立传递及两处 unreachable 删除已完成；验证同名／重复键、未知字段、认证／body／目标查找错误顺序和节点允许域。ApiPath 与 NoteTarget 的类型化 ID 构造仍未完成，后续保留原错误码和先后顺序；文本棘轮按现有函数更新 server json_value 277→278、wildcard_arm 86→87、unreachable 58→56，未本地编译、测试或运行 formatter。

- [ ] 定向 CI 验证 T07 object-move 目标槽位边界：完整 body 解码后构造持有 SlotId 的 MoveDiskObject，经过源／目标盘检查后一次转回原 engine 请求；验证大小写、64 字节上限、非法槽位、可空／覆盖目标、原 lookup 拼写、wire／canonical 字节和请求摘要。server 文本计数 validate_fn 34→33、parse_discard 34→35；新增匹配来自持有结果的 target_slot_id let 绑定，计数规则保持不变。其余 T07 准入与公开 validator 迁移仍未完成。

- [ ] 定向 CI 验证 T07 foreign-plan 目标槽位边界：ForeignDestination 从原 TryFrom 位置保留 SlotId 至 engine 映射，维持槽位先于 parent 的错误顺序和原始 history_names 延后准入；覆盖大小写、64 字节边界、原父路径与命名错误源，并运行既有 foreign-plan 管理行为。server 文本计数 validate_fn 33→32、parse_discard 35→36，新增匹配为持有 slot 的 let 绑定；其余 T07 迁移继续保留。

- [ ] 定向 CI 验证 T07 offline foreign-chain 槽位准入：持有 SlotId 完成前驱精确比较与大小写折叠碰撞检测，保持链基数／槽位／链接错误顺序、资源上限与持久记录；engine validate_slot 公有再导出暂保留，另轮降可见性并依据完整消费者编译证据处理。核对 zxs/core 行为、engine 产品图及已登记 helper／perf 消费者；server 文本计数 validate_fn 32→30、parse_discard 36→38，后者两处为持有结果的 let 绑定。其余 naming 公开 validator 与 manifest 准入迁移仍待完成。

- [ ] 定向 CI 验证 T07 Web recursive-backup capture 名称边界：Disk.name 在原字段解码位置构造并持有 Name，移除私有校验后回传 String 的 helper；验证既有 capture 的 canonical 原字节、Unicode／240 字节展示前缀、保留名／NFD／超长拒绝及原递归保护 UI 行为，并编译 WASM 产品。web 文本 validate_fn 87→86，其余指标不变；其余名称与槽位消费者及 manifest 准入仍待逐域迁移。

- [ ] 定向 CI 验证 T06：PortablePath 删除隐式字符串 Deref／PartialEq 后，naming／contract、engine、server 与 perf 消费者显式使用原文本；验证命名、namespace／alias／history 回归与相关 native／WASM 编译。真实来源摘要已更新，文本棘轮无变化；未以静态审查替代编译闭包或全部集成测试。

- [ ] GitHub CI 验证 T15 首批四种布局名字的 EntryName 接线：disk.json、writer.lock、.writer.lock、journal.bin 保持原字节／目录／锁用途；验证持有 fd、权限和链接拒绝、租约撤销、读取上限、原子替换与同步尾。其余布局名字尚待分轮迁移。

- [ ] GitHub CI 验证 T05 Generation／Sequence／Principal／Timestamp 定义和既有固定字节向量：完整 u64 字符串、MAX 耗尽、主体非空及原 Unicode／空白、Unix 整秒和 epoch 前错误来源。两个集中 next 保留可失败 checked_add；尚未迁移使用点或执行 T16 时钟迁移。

- [ ] GitHub CI 验证 T14 两计数 ReadView：Engine／终态视图仅借用 sequence 和 topology_generation，16 处生产读取保持原鉴权、锁与事件迭代；验证真实发布、重放、冷开、终态与 ready／topology 快照及借用约束。其余实体／收据读取和测试投影钩子仍待分轮迁移。

- [ ] GitHub CI 验证 T04 Id<K>／OperationId 的原编码、排序、serde 收据字节、解析错误与熵错误兼容；FileNodeId 五变体仅作基础类型，逐入口接入须保留原允许域。确定性 derive 尚须核实既有领域派生协议，不擅自引入新算法。

- [ ] GitHub CI 采集并审查 T03 的 quality／smb-sdk 完整 Clippy 警告报告，提交真实基线后验证精确八类、第一方包范围、计数增减与其余诊断严格门禁；缺基线仍失败，完整性为 false 的报告不得用作基线。验证 workspace lint 继承、原有 unsafe 拒绝与最小 FFI 例外。

- [ ] 完成《禁止防御性编程》§13 阶段 0：验证过程入口与文本棘轮，建立 workspace lint、Clippy 阈值及警告数棘轮；第一方继承规则保留 smb-vfs 自有许可证与最小 FFI deny 边界。
- [ ] 清点并逐类替换读取第一方源码或绑定测试名的 CI 门禁，改为行为验证；保留 foundation 依赖图、Cargo 元数据、来源与许可检查。
- [ ] 按 §19 的新版 T04–T07 完成 contract／naming 类型地基：`Id<K>`、多前缀 ID、`Generation`／`Sequence`／`Principal`／`Timestamp`、名称构造器及路径去字符串退化；补齐 `Hash<D>`、`TrackedMap<K, V>` 与 `EncodedLen`。
- [ ] 按新版 T08–T09 推进 HTTP 提取器、类型化备注目标、Web 操作号、会话 DTO 与 `SetupCall`，继续逐领域迁移 Web Endpoint／响应 DTO；保持现有未知字段错误、响应关联和凭据代数边界。
- [ ] T09：以 GitHub 定向 CI 验证 `SetupCall` 首次设置请求切片的 wire、未知结果围栏、会话代数与管理读取队列，并完成 Web WASM 编译检查；以定向 CI 验证登录／恢复会话 DTO 对响应接受域、迟到结果与错误来源的保持，以定向 CI 验证操作号／磁盘号直接从浏览器随机字节构造、原 wire 编码、未知结果操作号稳定性与既有会话围栏。
- [ ] 按新版 T10–T11 保留并传递现有 `AdmittedState` 证明、提升本状态访问器；完成 `BusinessState`／类型化 `AuthState` 与授权收据读接口，逐领域迁移 server 的裸 State 读取。
- [ ] 按新版 T12、T14–T16 推进错误显式映射、计数查询、`persist/layout` 句柄名字及确定性计划时间；迁移 serde 表示时完成真实快照／收据字节往返和摘要固定向量。
- [ ] 逐 kind 完成 `Command → plan → linearize → publish`，收敛 `Activity`／`Availability`、`BranchScope`／`DirectoryComposition`／`PolicyScope` 与文件属性聚合；先核实可达状态及写集成本。
- [ ] 完成领域错误枚举、按领域公共导出与模块结构收敛；公共项先降可见性，再依据完整 CI 目标／特性矩阵处理。
- [ ] 完成 platform 故障注入集中、业务 trait 接缝和产品／注入两档测试；清理随已删除防御分支失效的测试，保留准入、对外行为及真实字节验证。
- [ ] 完成 SMB 请求字段、auth 秘密类型、Web 表单／视图与 helper 输出的类型化；保留普通职责、算法、锁与生命周期注释及现有产品文档。
- [ ] 为 §15 与 §20 的待定事项逐项准备 ADR 并取得明确确认后实施：panic 围栏、HTTP 故障状态、EDQUOT、server core、FileRecord 持久形态、engine 拆 crate、Plan／收据表示、SMB 绑定代数位置、共享端点目录及 view 宏格式化方案。

## 目录、文件与移动

- [ ] 校准 [分支系统逻辑 §23](docs/design/branch-system-logic.md) 的单容器描述，区分同所有者路径移动与跨已提交目录／仓库的完整历史复制及独立身份规则，落实跨仓库复制要求。
- [ ] M01／M09：完成 [目录仓库](docs/design/directory-git-repositories.md) 的整体功能验收：空仓手动初始化／首空提交、保存／暂存／用户提交分离、父子独立提交、显式子提交引用、分支创建／改名／默认切换及只读工作树／提交快照导航。
- [ ] M01：完成独立 main 森林、脱挂／重新挂载、父 checkout 祖先身份、同名新身份、普通目录 scope、冻结组合、子仓归属与保旧实体布局的运行验证；覆盖冷恢复和完整文件归属。
- [ ] 验证普通 copy 仍创建不继承源历史的新文件；与跨已提交目录／仓库的 move 按完整历史复制并建立独立身份的规则分别验收，不混用两者语义。
- [ ] M01／M06：完成文件与整目录同盘／跨盘 `object.move`、空目标／非空目标、显式覆盖、已提交所有者间完整历史复制、独立身份与独立 FULL 副本的端到端验收；补齐仍受限的递归历史导出、分散正文、未物化继承及策略迁移。
- [ ] 验证移动的源／覆盖 occurrence、范围外别名、逐路径 context／节点、原目标 D 与准确恢复、隐藏目标额度、完整普通目录历史闭包；覆盖源冻结视图、旧读取句柄、目标兄弟项与两端权限。
- [ ] 验证共同移动执行器的 intent／choice／completion、首槽与归档缺失边界、Current candidate／membership 晋升、FILE／DISK／SYSTEM 全范围安装及一次发布；覆盖取消、丢响应、损坏 FULL、阶段中断、冷开精确重放与无重复发布。
- [ ] 验证目录路径／提交图投影、批来源持有、文件创建结果、State 跟踪／发布字段、冷热回执扫描、生命周期索引和同源投影复用，保持原身份、引用闭包、预算与持久字节。
- [ ] M03：完成 [共享文件历史](docs/design/shared-file-history.md) 的跨历史域关联需求及永久节点删除后的权威解绑／拆分；验证内容匹配、传递联合图和真实文件／节点身份。
- [ ] 完成目录高级操作的真实 API 缺口评估与实现：任意旧节点建分支、目录合并、已提交节点说明修改；核对目录标签缺失身份场景。
- [ ] 验证 Web 文件／目录导航、刷新、提交说明保留和非法输入拒绝、目标逐级浏览／分页、移动完成去向、取消及迟到回调隔离、Back／Forward。
- [ ] 验证 Web 分支树与控件外观：统一行高、输入下方无额外线条、下拉边框约原来两倍且略深于背景、背景保持；节点两行圆角卡片约十个英文字符宽，备注在上、彩色动作图标在下，共享节点以平滑虚线连接。

## 删除、永久退役与审计

- [ ] 完成单文件、多选文件、初始化 owner 内普通目录整体软删除及准确撤销的 Engine／HTTP／Web／SMB 验收；保留稳定身份、原路径行、HEAD／index、全部正文和永久 D／undo 审计。
- [ ] 验证批量别名只生成一个文件 D、完整 frontier／回执预算、失败无部分发布、Current selected tree 与 CAS tree 区别；目录撤销只恢复本次创建的 D，保持先前删除和其他活动目录 D 的引用关系。
- [ ] 补齐初始化仓库实体／嵌套初始化子仓库、跨仓库批次和原子整批撤销的剩余需求；分别验证原批次撤销清单、准确原目标恢复及路径被新身份占用时的拒绝。
- [ ] 验证当前／备份 listing、名称查找、已打开 SMB 目录分页与删除列表的生命周期可见性；原始保留行、冻结树、当前稳定目录身份与冷重开一致。
- [ ] 完成已实现的超预算单文件 D 分页／紧凑恢复剩余验收：server 紧凑投影单测、WASM 编译、真实浏览器和大规模成本。准确提交 `b07dd3da` 的 [#1188 定向 CI](https://github.com/isarmg/zxs/actions/runs/42993088091) 已执行通过 contract 4 项、Web 69 项及 HTTP 4 项，覆盖摘要、4097 分支与完整父边分页、永久完整收据及冷恢复、主体／新 D 隔离、原操作恢复和未知结果围栏；不等于全量或后继提交通过。目录／整批 D 仍沿原预算和恢复流程。
- [ ] 完成永久节点退役与共享关系拆分的剩余闭包：D／namespace 共同终止、原副本退出／换代、独立旧备份防复活和全记录物理删除；复验末节点／保留分支终止及 FILE 冷重建。
- [ ] 完成 [保留审计复制](docs/design/retained-history-copy.md)、公开单历史包及离线材料的完整 operation／principal／D／undo／退休凭据闭包；接入 Foreign 的 batch／directory delete、copy／move、原子替换和退役审计，保持源审计且不继承权限。

## 备份、数据库与恢复

- [ ] M02／M07：完成 [递归备份](docs/design/recursive-backup-file-publication.md) 与 [整体迁移](docs/adr/backup-target-replacement.md) 的多下级扇出、父子整体同盘迁移及最终换盘切换；每个真实目标保有完整独立副本。
- [ ] 验证 Backup 唯一原请求前的源序列／完整拓扑／目标空目录准入、未停写未决定取消、worker stop／join、保留式归档、原 op tombstone、取消中断恢复及明确新任务。
- [ ] 验证 Backup common transaction、原 seal／selector／bootstrap、全体 writer 与 Current／SYSTEM／Config 交接、后继布局、原 stop 收束和 history 重建；覆盖在线 Resume、晚失败收尾、混合 selector 冷恢复及完成后缺件拒绝。
- [ ] 验证 Backup 根记录、不可变材料配方、隔离目标写入、完成记录、后台 owner／取消／Drop、观察入口和共同成员决定模型；逐目标损坏／缺失仍拒绝，保留源围栏、原根和真实数据。
- [ ] M04／M05：完成 FILE、DIRECTORY、盘根 current／history 与 SYSTEM 三范围权威协议、全对象／别名／副本位置目录及 HTTP／Web 查询；验证每盘真实完整数据库和任意单盘独立恢复。见 [系统变更库](docs/design/system-change-database.md)。
- [ ] 验证 SQLite 连接／冷镜像生命周期、页额与有界读取、schema／完整性／引用闭包、事务和关闭双错误、SYSTEM WAL 原连接读回、选中分支准入，以及完成后损坏保旧拒绝。
- [ ] 验证真实日志的完整历史枚举、Backup 原盘／新目标首像与后继、准确 publication 序列、checkpoint 与自动历史维护的独立计量；测量最大规模的内存、FD、锁等待和同步成本。
- [ ] 验证成员 Add／Exit／readmission／terminal resume 的权威、代际晋升、stop 与混合协议残留拒绝；保持真实外部 owner 排空、同锁凭据复核及冷恢复完整性。
- [ ] 验证布局 locator 的当前位置／最终目标分离、双位置正文清理的顺序和父目录同步尾、缺失 owner／硬链接／替换拒绝、停止盘零访问及原操作重试。
- [ ] 完成取消用户新发起“离线／计划下线”的剩余入口；保留注销／移除、物理失联检测、安全围栏及既有未决操作的诊断与收束。

## 辅助工具与外来链

- [ ] H01：验收独立 root helper／无登录 Web 控制台的全生命周期离线租约、同版内嵌程序边界、同源／CSRF／IP Host、单任务 owner、退出排空与交接。显著提示 `0.0.0.0` 无认证 root 控制台仅用于可信隔离网络。
- [ ] H01：验证大响应 permit 从 worker 到 Bytes／HTTP 队列的连续所有权，覆盖慢客户端、断连、取消与关闭；核对小响应队列及连接数的剩余资源上限。
- [ ] H02：完成首账户／系统初始化、加盘、注销／移除、外来盘添加和磁盘初始化迁入 helper 后的真实离线闭环；验证五类协调器、原计划／摘要、worker 身份、进程租约及原请求恢复。
- [ ] H02：验证历史停止／注销根缺席、不可读／链接、挂载别名、未知目录、失联活动根、预览后换挂载及 Add 冷恢复；新根保持完整链和真实目录身份准入。
- [ ] H03：核对并整合保全的 Foreign 候选 `88942c48`／`14608eb2` 及后继，补齐真正单次接纳；四级及以上链按最多 32 级同批提供全部 N−1 个真实低级根，验证连续层级、原物理根和独立副本。
- [ ] H03：完成来源域命名／typed 图、当前池完整 State 绑定、原 seal 保留式 pending enrollment、全链 FULL 与目标三范围数据库安装读回、Foreign Joint 共同决定及活动／冷恢复收尾；接通同批协调与 helper 原请求，逐层提前启用保持拒绝。
- [ ] H03：补齐多原域、后继目的池、完整 D／undo／copy／move／原子替换／退役审计及降级材料的需求；完成后缺件、旧 ID／槽冲突和不完整归档保留拒绝，原历史不导入当前权限或伪造当前幂等回执。见 [Foreign 共同协议](docs/design/helper-foreign-joint-protocol.md)。
- [ ] H04：完成正常 ZXS 启动／运行对全盘、数据库、标签、正式接纳状态和未知外来材料的一致性门禁；保留损坏／终态管理诊断，完整初始化前不进入正常服务。
- [ ] H05：完成离线检查、可验证文件导出、系统盘汇总重建、精确中断恢复及任意单盘系统救援；补齐 SYSTEM 权限、共同 authority 与活动切换，验证三范围普通／救援准入。
- [ ] H05：完成终态改名／归属迁移后可信普通冷启动、serving／数据库闭包和共同活动切换；准确 generation 冷选择继续绑定原操作、epoch／日志及真实根。
- [ ] H06：验收管理员及真实 SMB 账户密码重置、首次缺失 tdbsam 账户登记、实际认证／会话撤销和外部写者排除；核对其他 Samba 后端／布局及管理员灾备副本同步需求。未知命令结果只查询原意图。
- [ ] H07：完成一条龙离线首装验收：Debian 13 amd64 依赖、独立非 root Unix 账户、同版 ZXS／Web／VFS、配置／运行目录、首管理员、1–32 个原批次空目录、生产存储与 systemd／SMB 交接。
- [ ] H07：验收 APT 刷新／解析／安装的原计划、包脚本服务动作隔离、部分 dpkg 状态及中断恢复；验证 HTTP-only systemd 与固定版本 Samba 两类依赖、双账户原请求隔离和真实 sysusers。
- [ ] H07：补齐既有配置替换／回滚、既有 VFS 升级／保留式替换；验证同版 VFS 真实安装、ABI 加载、网络登录／读写、原配置恢复、同载荷修复及文件／目录发布 inode 与同步身份。
- [ ] H07：完成 helper 页面真实 Chrome、普通目录转换离线向导、首装八步冻结输入、明确预览更正、响应丢失／刷新／重开、跨页与迟到响应验收；核对 helper 退出后服务接管和双向互斥。
- [ ] H08：完成内置同源码载荷与许可材料的实际构建／导出／安装验收，核对损坏主程序修复、保留备份与原摘要续作；明确任意历史安装修复的剩余范围。

## Web、SMB 与发行验收

- [ ] 完成 Web mkdir、移动、目录分支、单／多文件删除、目录删除／撤销和节点退役的原请求恢复验收：发送前保存读回、同账号刷新／重登录只查询、准确 ID／摘要／完整回执、明确重试、清理失败及存储损坏／冲突。
- [ ] 验证未知响应、查询缺失、续发拒绝、旧会话／旧代次／迟到终态、跨标签页竞争和恢复记录预算；验证 Backup Web Lock 取消／重启及同源原请求文本备份／恢复。
- [ ] 验证 Web 管理／遥测 DTO 的完整字段、null／零值／离线区分、大整数与预算、共享磁盘索引、失败保留旧快照、首次浏览及当前／历史请求关联；验证成员操作迟到 GET 不重开已接纳终态。
- [ ] 验证 HTTP Backend 单一声明、同锁身份准入、终态核心动作、上传／SMB 工作区单次准入、Job permit／AuthStore 所有权和凭证秘密错误；覆盖排队取消、撤权、停止、容量与 panic。
- [ ] 验证真实 SMB 默认／命名共享的 typed 路径／元数据／FD 所有权、帧预算、目录游标保全与 restart、只读创建空文件、Dirty／Flush／Close／Detach、丢回执和冷恢复；保留逐次账户／绑定授权。
- [ ] 验证 SMB 单一 namespace 操作模型、路径 reservation 的父子／大小写冲突、句柄脱离 Session 后持有、unwind 释放，以及统一 Rename／unlink／rmdir 的源身份、替换意图、未知 spool 保全与永久回执。
- [ ] 完成 M09 三级及以上备份盘正文读取、SMB 隔离、全部管理页和独立发布包的真实浏览器验收。
- [ ] 核对并完成 Release 四文件收敛：ZXS、helper、完整源码归档与 SHA256SUMS；BUILD／VFS／安装说明、全部来源与许可证保留在源码包或工具内，两个 ELF 绑定同次构建。
- [ ] 补齐完全离线发行所需 Debian 包／对应源／签名索引、固定 OCI、Rust／WASM 工具链材料，以及 SDK 缓存未命中的 apt／Docker 联网边界；验证下载复验、完整 Git 身份、真实 helper 导出与首次离线安装。
- [ ] 在获准环境完成物理掉盘／断电、真实目标设备、最大规模与性能验收；完成准确集成版本的全量运行后，另行取得发行演练、正式发布和真实部署授权。
