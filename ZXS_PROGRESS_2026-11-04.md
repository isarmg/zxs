# ZXS 开发进度快照

记录日期：2026-11-04，更新至 19:50 UTC。真实主线仍为 4e80fc4a；其 quality 作业 126811874735 已报告 15 项 lint/可见性错误，workspace 作业 126811874633 又报告 engine 18/server 2 项测试编译错误，未通过。下一源码批已修这些问题并整合终态管理、首次 TerminalResume、当前目录/历史消费者及普通 Add 定位账本恢复，尚未编译或测试。历史运行的统计时点分别列明。

本文记录本轮 ZXS 改进已落地的代码、已有验证证据、已知失败和剩余工作，供后续继续开发时核对。它是有时间边界的快照，不代替功能规范、发布说明或部署验收。提交和运行链接固定到具体版本；分支后续移动不改变本文所记录的历史结果。

## 恢复与当前命名空间修复（2026-11-04 22:07 UTC）

针对 `f4cff1a3` 引擎失败分组，本批修复 Exit 冷恢复基线、后续 Add 的 pending seal 和新成员准备目录、恢复前配额预检，以及当前命名空间复制和改名的来源证明。明确数据根恢复入口先验证固定元数据多数，数据根不计票，且要求事先保存的可信物理绑定，不从待接纳磁盘自证身份。挂载清单解析现保留合法 namespace 文件记录并禁止其作为存储目录；这与浏览器失败时序吻合，但原日志无清单快照，不能断定是本次唯一原因。

`333d4517` 的 [CI 41920400204](https://github.com/isarmg/zxs/actions/runs/41920400204) quality 暴露 9 条引擎测试 lint，本批一并修复；workspace 结果为 1,604 通过、49 失败、40 忽略，其他任务在本记录时仍运行。本地合并后的引擎库 `cargo check --offline --locked -p storage-engine --lib` 通过（后续测试模块 lint 修补尚未编译），之后补充终态错误来源包装及替换磁盘测试前提，并再次通过引擎库编译；各项新回归、真实浏览器和完整 CI 尚未复验；不能把修补数量等同于 67 项全数解决。


## 主线测试失败分组修复（2026-11-04 21:53 UTC）

`f4cff1a3` 的 [CI 41915719850](https://github.com/isarmg/zxs/actions/runs/41915719850) 已结束，quality、workspace 和真实浏览器任务失败。引擎 workspace 测试结果为 1,583 通过、67 失败、40 忽略。真实浏览器已通过匿名四尺寸及账户/激活响应丢失场景，后在挂载列表检查失败，原因仍待定位。

本批按实际错误修复已完成 Exit 被误视为未决转换的终态准入，保留关闭 guard 与投票证明；修正读尾取消、身份错误来源和单盘路由夹具前提，并补充回归源码。另修 Web 弃用 API 和服务端测试 lint。本地服务端测试目标和 Web 库 Clippy、引擎库编译检查已通过；本批其余测试未执行，67 项失败尚未逐项解决或复验，不能视为交付。


## 主线质量检查后续修复（2026-11-04 21:36 UTC）

`f4cff1a3` 的 [CI 41915719850](https://github.com/isarmg/zxs/actions/runs/41915719850) quality 已越过上一批服务端错误，随后发现 Web 弃用接口及服务端测试中的 5 项严格 lint。本批改用当前存储接口，并消除测试中固定分块与单元素切片警告，保留断言。本地服务端全部测试目标 Clippy 和 Web 库 Clippy 均以 `-D warnings` 通过；这不是运行测试或完整 CI 通过。原运行其他任务仍在执行，新提交需重新验证。


## 主线服务端严格检查修复（2026-11-04 21:23 UTC）

`e7163b60` 的 [CI 41913446289](https://github.com/isarmg/zxs/actions/runs/41913446289) 已越过先前引擎诊断，但 quality 报服务端 17 项严格检查错误。本批缩小磁盘观察错误类型，同时保留底层错误链，调整 SMART 固定数组分块及两处路径借用比较，并补充错误分类回归源码。本地 `cargo clippy --offline --locked -p storage-server --lib -- -D warnings` 通过；本地定向测试在构建过程中因磁盘空间不足主动中止，未得到测试结果。新提交的完整 CI 尚待执行，先前运行的其他项目以各自实际结果为准。


## 主线质量检查修复（2026-11-04 21:09 UTC）

针对 main `56a52638` 的 [CI 41903042460](https://github.com/isarmg/zxs/actions/runs/41903042460)，本批收窄终态激活函数的可见范围，删除未调用的内部辅助函数，整理终态接续边界参数并修正范围写法，同时修复两个引擎测试文件中的迭代与状态引用编译错误。原运行的 quality、workspace 和 Standalone ELF 失败；后者构建成功但诊断检查拒绝了相同的三条警告，并未进入后续浏览器验收。本批修改尚未执行构建或测试，需以本批提交的 CI 结果为准。


## 当前结论

- UI、SMB、历史、放置配置及中立失焦修补已进入主线 [`e54281cf`](https://github.com/isarmg/zxs/commit/e54281cffca72915287fd72caa0a16702fccfb0a)，保留原 `a8e57f0d` 祖先。磁盘基础随后进入主线 [`349367a3`](https://github.com/isarmg/zxs/commit/349367a37d5ab220dda1b66f3f17eb1cea44de03)。生产 Abort/recovery 与已有池添加 API/Web 随后进入 [`024647ca`](https://github.com/isarmg/zxs/commit/024647ca57f5824c6d0ab27a7691b019c087118d)。该提交的 [完整 CI 41876682031](https://github.com/isarmg/zxs/actions/runs/41876682031) 已在多个构建组报告 E0425，未通过。窄修已进入 [`1d082cd9`](https://github.com/isarmg/zxs/commit/1d082cd99f34e05ea52d85152d75590e7462df65)，[完整 CI 41882962028](https://github.com/isarmg/zxs/actions/runs/41882962028) 截至本次记录仍在运行；quality 因未使用的 `open_optional_regular` 和可合并条件失败，workspace 在测试调用不存在的方法处编译失败，其余任务尚未核实终态。三项源码修补已随真实退出批进入 [`57464e4e`](https://github.com/isarmg/zxs/commit/57464e4e118440708183bc43dbdf2709f27b10fb)，但该提交的 CI 又被下述编译错误阻断，不能标为通过。
- Web 首次设置已经有真实 Chrome、生产 WASM 和真实服务端的通过证据；同一轮浏览器验收随后在搜索框失焦截图检查处失败，不能称为整体浏览器通过。
- 已有池的在线新增、计划下线和删除登记尚未完成。实验协议、I/O 所有权收束及测试代码的进展，不能表述成这三项能力已经可用。
- `349367a3` 的 [完整 CI 41868114575](https://github.com/isarmg/zxs/actions/runs/41868114575) 已出现质量任务及 engine 测试失败；浏览器构建也被相同无消费者封装警告阻断，尚未进入浏览器。修补已进入 `024647ca`，但新 CI 被另一处 E0425 构建错误阻断。此前失败不能改写为通过；本轮没有真实部署或真实数据盘操作。

## 已合入 4e80fc4a 的主线构建窄修

主负责人已提交并读回 [`e533750a`](https://github.com/isarmg/zxs/commit/e533750a417002b4ccda75901a936145e0b0ed1d)，其 [完整 CI 41893637003](https://github.com/isarmg/zxs/actions/runs/41893637003) 实际报告：server 的 `DeregistrationPlanError` 不能转为 `ApiErrorSource`（E0277），以及 SMART 超时余数判断触发 Clippy `manual_is_multiple_of`。这些构建未通过，不能沿用此前结果或推断后续测试运行。

本次窄修只处理两项：退出预览将领域错误穷尽映射为原有具体 HTTP 错误码并保留 typed source/detail，附映射源码回归；SMART 用标准整数整除判断表达相同的毫秒向上取整，保留非零与有界内核超时。不加 lint 豁免，不降低证明或错误边界。

窄修已由主负责人提交并读回 [`4e80fc4a`](https://github.com/isarmg/zxs/commit/4e80fc4a04155e9b8d6d149c413806bd81e52f2d)，完整树 `f3d7eccd0b8cad85e3bb039529265f4c5f2967bc`。代码作者仅执行官方 rustfmt、差异及对象哈希核对，未编译或运行测试；其后新 quality 错误见下一节，不能记为通过。该补丁不含终态管理、TerminalResume、重新启用、目录选择/历史合并的后续源码或临时 CI。已有功能仍需对应验收，当前不作可部署结论。

## 19:50 待交付源码：终态与当前消费者

以准确主线 `4e80fc4a04155e9b8d6d149c413806bd81e52f2d` 为基线，仅以下源码已经整合，不代表执行通过：

- 最后一个投票数据目录退出时，保留原正 N/Q 和完整旧多数控制证明，先持久 SYSTEM archive 与受认证 stop，再等待实际 I/O Closed，进入无活动 metadata voter 的 TerminalManagement；不构造零盘 Genesis，不抹数据或凭资源表缺项判完成。HTTP/Web 已写终态实际交接、永久回执和冷启动选择；内容健康不虚报 Healthy。
- 首次从 TerminalManagement 指定全新 metadata 目录组 Resume，使用独立已绑定旧终态的 proposal/decision、完整控制 archive、新组规定 Q 和真实首个 publication，恢复当前 registry/occurrence 及历史、永久旧操作结果。普通 Add 不能伪装 TerminalResume。服务只在新组真实 publication 与永久操作结果完成后返回成功，live pending持有原SYSTEM锁；fresh目录发现排除已停止root/已知bind别名，但外部并发mount变化仍有未验窗口。新组公开 API、原添加通道的真实服务/Web接线和受限目录发现均已写并整合；取消、终态旧 Disabled 身份启用和再次 terminal 的 head 轮换尚未全部闭合。
- 普通 Add 与 TerminalResume 都有精确 bootstrap root 扩展计划：系统先保存准确前驱和目标记录，再写 record/head；恢复只续相同认证计划，不扫描选择最高记录或放宽常规启动。普通 Exit/Terminal stop 的同类中断计划仍在独立修补，不宣称全部掉电路径闭合。
- 当前选中 directory tree 不等于 branch head 的合法批量操作已写精确来源消费者；显式文件历史合并已写当前 owner occurrence、保留正文、永久 actor/digest 与回放验证。未选中的不等价 frozen parent 仍不自动导入。
- 已写独立终态对抗源码，覆盖重哈希 authority/head/archive 篡改、旧停止记录、原 journal 缺失、错误或回滚 selector、同逻辑身份第二物理域仍有句柄等情形。它们尚未运行，不能作为通过证据。

主线 `4e80fc4a` 的 quality 作业 `126811874735` 的 15 项错误按真实调用修补：`VerifiedProposal` 三个原始证明字段收窄到 pending 作用域；删除已被 `CurrentNamespaces` 替代且没有生产消费者的借用式 WriteProjection 原型，把其测试迁到真实当前 namespace/draft/upload/workspace 入口；合并等价条件。没有 allow、空调用或测试专用生产分支。源测试仍保留隐藏身份/版本、旧工作区、错 scope/view、历史字节与实际文件不变等边界。

workspace 的 20 项实际编译错误已按当前真实接口修补源码：普通 Engine::open 本来就执行 CompletePending，移除不存在的模式实参；历史 SharedRecords 使用真实 iter；joint 校验真实 certificate_hash、双方规定多数和已安装覆盖；日志损坏回归用该 Engine 的准确代际 log_path；Note 传拥有的 String；可选受管 ReadFile 错误直接匹配，不为测试增加 Debug。保留原拒绝、回执、历史与目标字节断言，另清理本批新增历史合并回归中的同类调用误用。async 与 SMB 的实际终止也是相同 server 两项 Note 编译错误，前段 Samba IDL/panic 是预期负例，不列为新增根因。

本次代码作者仅执行源码阅读、合并、官方 rustfmt 和差异核对；未运行编译、Clippy、任何测试或 CI。下线／删除／恢复管理的完整功能验收尚未完成。活跃池 Disabled 重新启用、stop publication 中断、重复终态权威及当前命名历史分支消费者继续各自在独立范围编写，由主负责人统一提交和组织验证。

## 17:53 主线构建阻断与窄修

- 生产 Abort/recovery 与添加 API/Web 的 60 文件源码批已进入主线 [`024647ca`](https://github.com/isarmg/zxs/commit/024647ca57f5824c6d0ab27a7691b019c087118d)。其 [CI 41876682031](https://github.com/isarmg/zxs/actions/runs/41876682031) 多个构建组报告 E0425：激活发布调用 journal 私有的 `write_object_bytes_at`。这些构建没有通过，不能沿用旧结果。
- 本次只新增 `PreparedMetadataObject::store_at_root`，让已哈希的规范对象字节通过实际持有的 `DiskRoot` 写入，并迁移激活调用。不扩大原始 writer 可见性，不改格式、对象哈希、原子写和不可变碰撞检查。附幂等、损坏不覆盖和不重新编码的回归源码。该窄修已由主负责人提交至 `1d082cd9`；代码作者仅完成格式与差异检查，新验证结果待主负责人核验。
- 后续计划下线/删除登记、连续代际恢复、当前列表和写入投影仍在独立源码批开发，不包含在此窄修，也不因主线修复而宣称已完成。磁盘指已登记的存储目录；没有真实设备操作或部署。

## 最新工作顺序

最新分工是代码作者继续编写实现，主负责人统一提交和组织测试。代码作者不启动测试、构建或新的验证分支，不推送远端；主负责人按冻结批次整合 `main` 并记录实际检查结果。保留正式工作流和质量门禁，不删除或弱化它们。此前暂停测试及统一使用 `[skip ci]` 的安排已由此更新。

代码完成、提交到主线和执行验证分别记录。未执行的新修改保持“未验证”状态；合入主线本身不构成功能可用、CI 通过或可部署的证明。后续若恢复验证，应更新具体提交、命令、环境和结果，不沿用旧提交的结论。

目录一键同步不在本轮继续推进。当前优先收拢 ZXS 的既有改动和磁盘生命周期缺口。

## 已落在主线的相关基础

以下是代码已进入主线的事实，不表示所有用户流程都已验收：

- 磁盘健康、SMB 账户控制及自适应 Web 的一批改动，见 [`3608e310`](https://github.com/isarmg/zxs/commit/3608e310361e9bf05b209b0572037b72a87a8936)。
- 无密钥首次设置、首个存储目录激活及分支浏览，见 [`8e766718`](https://github.com/isarmg/zxs/commit/8e7667187e420a2fdf28a791205162d6fa53c569)。首个目录激活与已有池的在线增删不是同一能力。
- 登录和首次建账户的六行布局，以及匿名登录错误装饰收敛，见 [`c04595eb`](https://github.com/isarmg/zxs/commit/c04595eb973642544096ccd992c9191a287d6e13) 和 [`b80d51a0`](https://github.com/isarmg/zxs/commit/b80d51a007cb00f40b82ceab7336e10f1b89a72e)。
- 已持有正文文件的分块取消检查，同时保留身份、长度、摘要和单链接校验，见 [`a8e57f0d`](https://github.com/isarmg/zxs/commit/a8e57f0d71eb52df219fdbce1f5a722ee13c9d7b)。它没有开放磁盘生命周期操作。

`a8e57f0d` 的 [完整 CI 41837522717](https://github.com/isarmg/zxs/actions/runs/41837522717) 已结束，结论为失败。静态工具、production WASM、各 Rust 测试分组和 Debian 13 Samba 任务通过；`Standalone ELF: Debian 13 server + host Chrome` 失败。不能把这轮主线记录写成全绿。

## UI 与 SMB 历史功能批次

公开验证分支为 `validation/ui-smb-history-20261104`，快照时指向 [`ca95059b`](https://github.com/isarmg/zxs/commit/ca95059b07c1a37eb78114242430988fb6bf12d1)。本批组合了登录和首次设置布局、文件行与搜索焦点、默认 SMB 共享、文件历史合并、目录历史树及放置配置等改动。详细边界见该提交的 [批次说明](https://github.com/isarmg/zxs/blob/ca95059b07c1a37eb78114242430988fb6bf12d1/docs/validation/ui-smb-history-batch.md)。

### 已有通过证据

[`ca95059b` 的运行 41857611231](https://github.com/isarmg/zxs/actions/runs/41857611231) 中：

- 来源与工具静态检查、Web 状态和样式合同、production WASM 构建通过。
- 真实浏览器在四个视口完成首次设置的正常、限流 429 和未确认状态布局检查，反馈和操作控件保持在设置卡片内。
- 无磁盘创建管理员、管理员监控、真实嵌套目录浏览与激活、配置根核对及目录缺失后重启检查通过。
- 真实激活过程中响应丢失或进程中断后的原操作恢复、既有账户登录和权威不变检查通过。
- 文件夹与文件行的部分真实悬停、选中、键盘焦点、横向滚动和响应式截图检查通过。日志还包含明确标注为合成数据的分页与取消探针，不能用它们声称真实大规模性能通过。

该轮 native 和 Samba 任务标为 `skipped`，使用了先前 [`72e7258b`](https://github.com/isarmg/zxs/commit/72e7258be1d1a5d461476c259db5ec0eeec7e75f) 的 [41839207213](https://github.com/isarmg/zxs/actions/runs/41839207213) 结果及对应来源约束；不是在 `ca95059b` 上重新执行 native/Samba。

### 已知失败与未运行修补

同一轮真实 Chrome 任务最终失败在 `search-focus` 的失焦 PNG 检查：搜索框已失焦，但 Shift+Tab 后相邻“备份设置”按钮的焦点轮廓进入截图区域，角点和曲线像素断言失败。圆角像素采样此前也需要处理实际曲线与对角抗锯齿位置的差异。这些问题必须区分为产品状态与验收探针行为，不能通过放宽颜色阈值掩盖。

新的修补代码把纯失焦截图与键盘焦点往返分开：先真实点击无动作的中立区域并确认焦点，再保留 Shift+Tab、Tab 返回搜索框的严格检查。修补已完成源码审阅，但尚未执行。它已随 e54281cf 提交，但不能写成修补后通过。

`e54281cf` 已收录 UI 候选及中立失焦修补；公开验证分支仍保留 ca95 的历史结果。该主线提交带 `[skip ci]`，读回未产生新运行。新修补未执行，不能据此声称验证通过。

## 本次磁盘批交接

本次追加纯添加与成员协议基础、磁盘登记及影响预览、I/O 所有权和排空、checkpoint、repair/maintenance，以及 Abort 读侧源码。三方整合保留 UI 的历史、放置配置、默认 SMB 共享及测试接线；新增 Engine 导出与原 UI 导出并存。

同时包含旧测试夹具对新 JournalIo/持锁语义的修补，及最终 drain 后目标盘锁释放、系统锁仍阻止竞争的断言修正。d799 既有运行后来确认失败，七个不同用例涉及旧夹具假设；本次修补没有执行，历史失败不因此变为通过。

不纳入仅服务实验验证分支的 workflow、脚本及其专用 guard，正式 CI 保留。修复 I/O 测试说明集中到 [docs/testing/repair-io-tails.md](docs/testing/repair-io-tails.md)。基础批整合阶段只做源码合并、格式化和来源清单生成。该批后来由主负责人提交为 `349367a3` 并启动上述完整 CI；源码作者未执行本地测试或构建。

`349367a3` 基础批不含生产 Abort/recovery 与在线加盘 API/Web；它们纳入下述后续源码批。计划下线、删除登记、退出代际与最终管理闭环仍按未完成记录，不能把基础协议提交当成完整能力可用。

下一部分完成时更新本文件中的代码范围、提交、已知失败和未执行验证；不要新增相互矛盾的交接副本。

## 本次后续源码批：添加入口与可认证取消

本批以 `349367a3` 为基线，保留 UI 的历史、放置配置、SMB 和 `FileHistorySourceRead` 导出；未增加临时 CI 或削弱正式门禁。

- 已写生产取消协议：旧成员实际锁和不可变决定槽、固定旧多数认证、同一代际不同合法 witness 安装、仅原根推广、取消 publication、永久操作结果，以及 Add → Abort → Add 的恢复接线。旧 Candidate/Add 的既有字节和哈希未更改；新阶段不重置已有池。
- 已写已有池添加的 HTTP/Web 预览、提交、状态及原操作恢复，支持 data-only 和 mixed 目录角色；使用持有 FD 开始 fence、系统目录内受管控制归档、持久请求绑定、同操作幂等、并发互斥、当前会话复核和连续代际重启准入。
- 已补 metadata 读取、归档复制和决定槽的实际 I/O/同步/关闭责任尾部；删除两个生产无消费者的非同步删除封装，相关测试改用实际同步入口。
- 生产取消核心已写，但本批 Web 仍只提供继续原添加操作，不提供取消按钮。取消管理入口、真实计划下线/去登记、第一条目标 I/O 之前的启动禁止、已认证 registry 当前可写视图仍在后续并行实现。

`349367a3` 的 workspace 任务中，engine lib 实际结果为 1416 通过、5 失败、40 忽略。本批按日志处理五项失败：两项坏 marker 导致全盘拒绝是真实发现阶段回归，改为损坏副本隔离且仍要求同一权威的固定多数，不降低 N/Q、不跟随坏副本路径；两项旧 fixture 仍期望先获得无效 PortableAuthority，改为在严格反序列化边界断言拒绝；一项 cleanup fixture 实际破坏容器布局，拆成准确的布局拒绝与真实缺父目录 NotFound 两个独立场景。保留坏盘不修改、不得通过改配置缩小多数、正文/metadata/状态不变等断言。上述修补均未重新运行。

本批仅执行源码阅读、合并、rustfmt 和 git diff 检查，未执行类型检查、测试、模型、浏览器、SMB 或 CI。新增回归文件是待运行的源码，不是通过证据。需要主负责人对组合树运行 engine membership/Abort/recovery、server 添加与重启/响应丢失/撤权、Web 状态/生产 WASM 及正式质量检查；真实硬件掉电和部署不在本轮范围。

## 已合入 57464e4e 的源码批：真实退出与当前投影

以下是基于准确主线 `1d082cd9` 整合、尚未组合编译或运行的源码，不是通过证据：

- 已接入真实首轮计划下线／去登记：保留盘先独立持久化完整控制归档和规定 Exit authority，系统持久 stop 先于目标关闭；随后关闭普通准入、撤销空闲资源并等待实际尾部和锁关闭，只在保留盘发布 D3。新 registry 与完整 occurrence mask 绑定冻结源，实际 publication/journal/snapshot 重新核验后才激活当前视图。
- 启动路径在首次存储目录 stat/open 前应用受限 locator。陈旧系统 Config 不再自动探测已停止目标；缺失或不可信定位信息进入明确恢复路径。运行时的逻辑身份排除覆盖同一登记的所有物理 I/O 域，并保留到 Engine 生命周期，不能把当前资源表缺项当作已经排空。
- 已写当前登记枚举、Engine/MetadataView 当前列表和查询、普通文件管理工作区的投影基线、同名新身份、DeleteTree、持久工作区准入收据及逐发布恢复。历史 Commit/Version 不重写；隐藏项不应被普通后续提交重新带入当前命名空间。
- 已写添加的真实认证取消入口，以及计划下线／去登记的 HTTP/Web 确认、原操作恢复和永久结果查询。控制准备状态不是成功，激活中只续原激活而不重开被禁止目标。
- 已写受管目录观察和 SMART 实际进程尾部责任。超时返回或接收者丢失不等于子进程退出；无法确认清理结果时禁止宣称关闭。同进程的已关闭旧计划不得通过健康查询重开目标；服务进程崩溃后外部 SMART 子进程的存活与内核 I/O 尾部，尚无跨进程排空证明，不能据此宣称全面覆盖真实掉电或服务崩溃。checkpoint 新上下文绑定准确代际、活动 I/O 身份与固定 voters，任何 registry-bearing 代际仍从 D3 完整回放，不启用未经证明的加速。

当前 HTTP 文件管理、目录 Full/summary 和 SMB 已写已准入投影消费者；同一 immutable HEAD 下 registry 改变会要求当前目录句柄重新绑定，显式历史访问保留历史树。已写多代 Exit 后继续添加／退出、Disabled→Deregister、精确旧操作回执、当前 Rename、新仓入口和主盘配置路由；源码尚未运行。当前目录 scope 初始化、当前目录分支标签、纯元数据编辑，以及首次激活补全不能绕回已停止目录的连续锁证明均已写入同一候选，尚未编译或执行，未将 gate 当成功。该批冻结时，最后一个 voter 的管理终止权威、原参与者缺席时的准备驱动、merge/rebase/批量或历史导入，以及大规模分页便携证据仍未完成。下一节单独记录后续已写源码；未闭合阶段保留明确拒绝，不以按钮或准备证书伪造成功。

本批同时移除无消费者的原始可选文件打开封装，保留实际受管入口；等价合并 recovery 条件；将 Abort 回归改为真实 joint 续作并断言双方多数与证书，处理 `1d082cd9` 的三项已知构建／lint 阻断，没有豁免警告或放宽多数。

本批作者仅做源码整合、格式化和差异审阅。新测试源码涵盖第一轮 D3、stop/关闭/部分 publication 中断、原操作续作、目标字节保留、共享内容、旧工作区、逻辑域排除、观察取消与 checkpoint 范围；都需要主负责人针对最终同一树运行后才能记录结果。

## 已合入 e533750a 的后续源码批及构建状态

本批基于真实主线57464e4e，已由主负责人提交为 [`e533750a`](https://github.com/isarmg/zxs/commit/e533750a417002b4ccda75901a936145e0b0ed1d)。61文件净变化：源码32个 +3567/-334、测试15个 +4608/-16、文档14个 +343/-35，不累加中间提交。源码作者未运行编译或测试；该提交 CI 的两项新阻断及窄修见前文。

前一主线57464e4e 的 CI quality 任务 `126788359129` 实际报告未使用的 `DurableDiskStop` 导入、工作区 base 借用与隐藏 context 修改冲突、Exit runtime 两处 failpoint 的可变借用冲突、restricted recovery 中 metadata iterator 与移动 Config 冲突；workspace/build-tools 也在编译阶段被阻断，不能声称后续测试已经运行。本批改动将 base/current 绑定同一已持有 source-commit 快照，并修正上述生命周期借用范围；保留故障点、真实 guard、多数与持久化条件，不加入警告豁免。修补尚待运行确认。

本批写入的实际消费者：

- 唯一退出 voter 缺席时，用原固定 N/Q 的完整已发布前缀及持久 fence 准备，完整安装所有保留目标；data-only 缺席沿用完整原 metadata 证明。Unopened 只有取得准确 durable stop、restricted authority 和全逻辑 I/O 域关闭证明后才可推进，不伪造缺席 journal 或零 I/O。独立 11 项对抗回归源码与原正负回归同时交付，见 [缺席退出](docs/validation/missing-exit-members.md)。
- 当前文件分支创建、选择、默认与合并绑定准确 owner/occurrence、历史正文和 child refs；当前目录分支创建/选择发布必要的不可变投影树，保留旧树。恢复重建相同来源证明，不凭回执布尔值放行，见 [文件分支](docs/validation/current-file-branches.md) 与 [目录分支](docs/validation/current-directory-branches.md)。
- 当前选中且 tree 等于 branch head 的批量操作已写 Move/Copy/Rename/DeleteTree 等真实消费者。选择投影导致 tree 不等于 head 的合法后续批量消费仍在独立实现，此冻结批精确拒绝而非借旧树放行，见 [批量编辑](docs/validation/current-namespace-batches.md)。
- 上传与同次 Rename 绑定真正新版本及最终路径，恢复拒绝多余别名/隐藏身份。删除链最终消耗上传时仍明确拒绝并保留暂存，因为尚无跨两类动作的用户顺序记录，见 [工作区写入与改名](docs/validation/workspace-rename-writes.md)。
- SMART 目标 I/O 改为服务内同步 NVMe/SAT 查询，保持真实准入直到内核调用返回。smartctl 只用 stdin 离线解释，无目标路径或设备 FD；来源解析、取消、超时及错误保留。跨进程/真实硬件验收未执行，见 [SMART 生命周期](docs/validation/smart-query-lifetime.md)。

尚未纳入此批的生产闭环：最后 voter 退出进入经认证管理态、从该管理态建立新 metadata 组、完整历史导入/rebase、选择投影后的批量消费者，以及更大规模证据分页。前两项独立并行实现，不能当作零盘 Genesis 或临时 N=0/Q=0。已有在线管理源码也仍需编译、故障恢复、HTTP/Web/SMB 和实际 I/O 验收，未获可部署结论。

## 在线磁盘生命周期与 I/O 收束

这里的“磁盘”指 ZXS 登记的数据存储目录，不等同于独立物理设备。本文没有挂载、格式化或系统盘管理进展。

### 当前代码方向

实验代码涉及代际与成员身份、fence、控制归档、pending 准入、联合决定、纯新增激活与恢复，以及磁盘 I/O 的准入、取消、排空和资源关闭。退出控制准备只形成准备协议，不能替代下线、删除登记或新的活动权威。

本轮正在补齐容器、发布、日志、修复、恢复检查点等实际文件 I/O 的所有权尾部，防止操作已经返回或取消之后仍残留未收束的文件、目录、流或写锁。必须同时保持历史证明、原配置多数规则、永久操作结果及失败时原数据保留，不能靠提前丢锁或忽略错误让测试结束。

### 已有失败与当前分支

- [`665344bd`](https://github.com/isarmg/zxs/commit/665344bd06dc54c141f1b8e3eb95eaf349fd5140) 的 [运行 41854805254](https://github.com/isarmg/zxs/actions/runs/41854805254) 最终为 `cancelled`。取消前 activation 分组 36 项通过，membership 分组 166 项通过、1 项忽略；scrub 分组 59 项通过、1 项失败，涉及重开池后旧批次与资源生命周期的测试假设。随后目录流测试因旧测试自身持有读锁而等待写锁，未正常结束。不同过滤分组可能重叠，不相加为独立测试总数。该轮既不是通过，也不能仅以“取消”省略已经出现的失败。
- 磁盘分支 `verify/membership-disk-io-20261104` 指向 [`d799a37f`](https://github.com/isarmg/zxs/commit/d799a37f3eb49d686166271fd41cbe5066726dc5)，包含 I/O 所有权尾部和卡死回归的修补。[运行 41860913932](https://github.com/isarmg/zxs/actions/runs/41860913932) 在快照时仍为 `in_progress`：工作流、架构与格式步骤通过，测试二进制编译及受影响分组步骤尚未结束；abort 模型、production/Clippy 和文档步骤尚未执行。
- 恢复检查点分支 `verify/membership-recovery-checkpoint-20261104` 指向 [`7bce9add`](https://github.com/isarmg/zxs/commit/7bce9add0dc237a4beb1af3f20d6adf9b8bbce15)，改动为把受管理的读取所有权保留到 I/O 尾部。[运行 41858184857](https://github.com/isarmg/zxs/actions/runs/41858184857) 在快照时仍停留于编译与受影响分组步骤，后续检查尚未完成。该支线不再独立推进，既有代码待纳入统一组合；不能把它当作已经验证或已经完成合并。

以上运行在最新“不启动新测试”的工作顺序之前已存在，新增修补仍按未执行记录。

### 尚未完成的能力

1. 完整在线新增流程：准备、恢复、取消或失败退出、激活、永久结果与管理入口需要共同闭环，不能只以某一核心实验阶段替代。
2. 计划下线：保留登记和内容可见性，同时停止目标目录 I/O，并在中断、重启及旧读者存在时保持准确状态。
3. 删除登记：从当前可写命名空间与列表中移除目标独占内容，保留应保留的共享内容和控制历史，不擦除正文，也不以生成恢复文件或备份作为删除条件。
4. 真实 abort 投票、marker 等价安装与恢复普通写已写入本次后续源码批，尚未验证；首轮退出代际、registry 与当前可写视图激活、启动前停止限制已写入后续源码但尚未验证，HTTP/SMB 当前投影、多代延续及首次补全防探测已写入同一候选，均未验证；退出期间缺席成员、最后一个 voter 的终止权威及完整 I/O 排空与关闭也不能由控制准备证书代替。
5. 上述能力的 HTTP/Web 管理入口与最终端到端结果仍未完成。临时目录和故障注入也不证明真实断电、硬件故障域、跨主机 writer 或真实部署行为。

## 失败原因与本轮调整

截至 2026-11-04 15:53 UTC，GitHub Actions 按以下两个分支筛选，记录 21 个不同提交对应的 21 次定向运行：

| 分支及最后一次运行 | 运行数 | 成功 | 失败 | 取消 | 运行中 |
| --- | ---: | ---: | ---: | ---: | ---: |
| `verify/membership-first-activation-20261104`：[41814971387](https://github.com/isarmg/zxs/actions/runs/41814971387) | 6 | 2 | 4 | 0 | 0 |
| `verify/membership-disk-io-20261104`：[41860913932](https://github.com/isarmg/zxs/actions/runs/41860913932) | 15 | 3 | 10 | 1 | 1 |
| 合计 | 21 | 5 | 14 | 1 | 1 |

恢复检查点支线的 `7bce9add` 和运行 41858184857 另计，不混入这 21 次。此统计只覆盖这两个分支的定向工作流，不是全项目提交数或完整 CI 次数。最近一次成功为 [`b2866d8c` 的运行 41846343311](https://github.com/isarmg/zxs/actions/runs/41846343311)。

已有记录显示，主要返工包括编译导入、类型与格式问题，旧测试夹具假设与新的资源生命周期不一致，以及目录流测试自身持锁造成的死锁。UI 另有布局反馈和截图探针问题。部分检查在更早阶段失败后，后续步骤根本没有运行。单项编译、静态审阅或前一提交的通过不能提前覆盖后一轮结果。

目录流死锁与旧 FD 断言修补已进入 `d799a37f`。scrub 旧测试假设另有后续源码修补，尚未执行，不能用它把上述失败改写为通过。本轮先提交现有 UI 代码与交接文档，再继续完成磁盘代码并分批整合 `main`。用户最新要求由主负责人执行提交及测试，代码子代理负责实现。继续保留具体失败和来源，不通过降低完整性条件、放宽验收阈值或把 `skipped` 写成通过来缩短进度。

## 继续工作的顺序

1. 主线已到 `4e80fc4a`，主负责人核验构建窄修后的新运行结果。代码作者继续终态管理、新组接续、重新启用及其当前消费者与断电恢复闭环，下一批保留真实主线父链。
2. 合并到同一候选树，保留各方实现，清理仅服务临时验证分支的专用门控和过时说明；更新本进度文档中的主线提交与状态。
3. 将已冻结代码提交 `main`，由主负责人组织受影响检查和完整 CI；尚未执行的检查仍标记为未验证，既有 CI 的最终状态另行补充，不自动推断。
4. 不进行真实部署、凭据或权限修改，不操作真实数据目录。在线新增、下线和删除只有在实现与对应验收都完成后，才能改写为可用。

## 证据使用约定

- 运行结果只适用于所链接的提交、环境与实际执行范围。
- `success`、`failure`、`cancelled`、`skipped`、`in_progress` 和“未执行”分别记录。
- 本页只保留公开提交、公开运行与必要的技术结论，不包含临时目录、内部工作路径、凭据或原始敏感日志。
