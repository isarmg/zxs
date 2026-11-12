# 按变更定向 CI

main push 自动执行 `targeted`，手动入口默认 `targeted`。只有明确选择
`scope=full` 才运行原完整静态检查、13 套件矩阵、workspace 分片验收、Web、
浏览器、SMB SDK 与 phase-one-core。Release、PR、schedule 保持原暂停状态。

## 选择方式

`.github/ci/targeted-tests.json` 保存持久的路径与行为模块映射。
push 使用事件的 before 到当前 HEAD，包含整个 push 中全部提交及删除路径；
手动 targeted 检查 HEAD 的一个父提交到 HEAD。映射按路径取并集，重复组只运行一次，
不依赖旧产物或固定基线，也不要求每次提交重写计划。新增未登记路径、无法读取的
历史、非祖先基线、空选择均会失败，需补充相关定向映射再运行。TASKS、说明文档与指标文件单独变化只运行轻量 CI 策略回归。

映射由维护者随职责变化审阅，表示已审阅的覆盖范围，不是完整的跨模块依赖分析。
engine.rs/lib.rs 等公共入口当前映射到本轮恢复、journal 与 IO 模块；未来修改这些入口
的其他语义时必须同步扩展映射，路径匹配本身无法判断语义。修改 Cargo 清单、特性或
新的子系统时应扩展相应的定向行为组，而不是自动启动全量。完整工作流与安全门禁
保留，可由明确的人工 full 请求执行。

## layout 与持句柄 IO 的定向范围

- engine persist/layout：初始化后冷打开、已有盘上文件名与字节、独立 writer 锁、
  目录改名后的持句柄读写、缺失/链接/读上限。
- disk owned/root_operation、directory_entries 与 admission：撤销与 quiesce、
  描述符释放、目录替换、边界读写；ignored 的隔离挂载测试单独列为未执行。
- journal writer 与生命周期：日志写入、writer 持有期及相关恢复行为。
- readmission、terminal_resume、backup startup resume、backup preparation、
  recovery checkpoint、history move choice：本次 layout 路径迁移涉及的恢复链行为。
- platform fs_safety：原子替换、已有句柄身份、锁、路径与字节上限。

每个模块从本次 Cargo 生成的测试二进制枚举，名称变化无需维护固定测试名。
每个选择前缀至少有一个非 ignored 测试；engine layout 最多 187 项，platform 最多 100 项。
超过预算需审阅分批，不能静默截断。每项以 `--exact` 执行并核对一项通过，零项、
ignored、最终父测试摘要不匹配、执行失败均无法完成本批。构建仅针对两个包的 lib 测试目标；
其依赖仍需正常编译。

## 结果

`targeted-tests-<sha>` artifact 记录比较基线、当前提交、映射摘要、完整当前库存、
选中及未执行名称、二进制 SHA-256、逐命令日志与逐项结果。状态
`selected-tests-passed` 只结算选中项。构建或执行中断时保留 incomplete/failed 记录。
全 workspace、其他恢复场景、ignored 外部 IO 场景、跨特性、Web/浏览器、SMB、
foundation、quality、security 都保持未执行，不能用本批替代发行或全量验收。

CI 路由和证据脚本变化只选择对应 Python 回归，自动入口不会先执行整个 static。
并发组仍按 run ID 隔离；已有运行的取消由用户单独决定。

## 扩展定向目标

每项可显式指定 Cargo 的 lib、命名 test 或命名 bin，features 与 default_features。
默认仍为本机 lib 测试。选择支持模块前缀、精确行为名或整个小型测试目标；每个
目标均受非空与各自 max_tests 约束，配置准入上限为 192 项。相同包的不同集成目标及特性配置分别绑定回执。

`mode: check` 表示单个指定目标的编译验证，不计入测试通过数。只有该项显式指定
`compile_target: wasm32-unknown-unknown` 时安装 WASM 标准库并执行 release check。
编译项保存当前 Cargo 成功回执及实际目标产物摘要；执行项仍保存二进制库存与逐项
结果。包、manifest、target 名称/种类与要求的 features 必须匹配当前构建输出。


## 重叠选择的构建与预算

重叠路径保留原有组。相同 package、kind/name、features 集合、default_features、
mode、compile_target 只构建一次；manifest 冲突失败。16 项上限按唯一构建配置计数。
每个原选择独立检查非空、ignored、模块存在性及 max_tests，再合并执行名称。
同一测试只执行一次，采用所有适用选择中最短的 timeout。回执保留来源组、各自配置、
各自选择结果与最终逐项 timeout，便于核对原范围。组合预算是这些独立预算的并集，
而不是给单个选择扩容；超过任一原预算仍失败。不同特性、平台或执行模式分别构建。

## 完整并集与超时隔离

自动入口先准入同一 SHA／基线的完整并集，再分为每片最多 16 个 Cargo 构建、
8 个 Python 模块及最多 16 片。原选择、库存、预算和逐测试 timeout 全部保留。

行为项可显式声明布尔 `isolate`，只改变安排，不是新的 Cargo 构建身份。只要任一
重叠项要求隔离，该构建及其全部合并选择就独占一个 Rust 分片；前后目标保持原顺序，
Python 仍按每片 8 个分配。原 `recursive_backup_paths` 备份事务独立隔离，
900 秒限制不变，避免其已知超时阻断无关 Web／契约目标的执行证据。

`portable-path-consumers` 的 `http_engine` 项也声明隔离，将这次选中的整个 HTTP
Cargo 身份及所有重叠来源一起放入独立分片，不拆散原选择。#1307 的实际执行中，
server 单元与 HTTP 集成之后，原第 0 片在约 82 分钟时才开始 Engine 单测编译。
此安排避免前方 HTTP 用掉同一作业的剩余窗口；每片仍是原 90 分钟，每个测试仍用
原始来源中的最短时限，完整并集、预算、执行顺序及总片数上限不变。它不是测试加速、
性能根因或旧运行的通过证明，也不取消或重跑原运行。#1307 最终在原作业时限后取消，
第 0 片收据为 incomplete；这次安排仍须新提交的真实执行验证，不能把旧收据的空失败
数组当成成功。

各片仍绑定完整计划摘要和准确提交。任一失败、取消、超时、未执行或缺失收据都会
使总验收失败；其他片通过不能替代备份成功，也不代表完整功能或发行验收完成。

## 类型化名称的定向范围

`typed-name-consumers` 覆盖 naming 的 `name_tests`、contract 的
`repository_name::tests`、engine 的 foreign import mapping/intent 与 historical save、
server listing 的查询名称等价性，以及 `http_engine` 的 `history_alias_operations`。
HTTP 集成目标显式启用 `core`，包含 source slot 拒绝与 alias 历史行为。
server 无默认特性的 native check 验证公开边界；web native 和 WASM release check
验证名称/路径类型消费者的两种编译图。实际测试名称由当前构建的库存确定。
