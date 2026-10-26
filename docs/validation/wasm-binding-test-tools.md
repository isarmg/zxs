# 绑定转换测试的文本工具

本轮保留 WAT 输入、独立文本 golden 和生产 Legacy EH，不用被测 walrus IR 构造
替换输入，也不把输出切换到不支持 Legacy EH 的 folded 模式。

## 当前职责与来源

| 当前职责 | 精确上游来源 | 实际用途 |
| --- | --- | --- |
| `wasm-text-parser` | wast 245.0.1 | 直接 ParseBuffer → Wat → encode；转换 directive；Unicode 定位 |
| `wasm-text-printer` | wasmprinter 0.245.1 | 仅独立无色普通文本 golden |
| `unicode-display-width` | unicode-width 0.2.2 | 解析错误 snippet 的正常 Unicode 终端显示列宽 |

工具正常依赖复用现有 bumpalo、leb128fmt、memchr、wasm-encoder、wasmparser、flag-set；anyhow 仅留在已接入的测试 harness 开发依赖，不进入新库实现。
encoder/parser 统一 0.245.1。没有 wat 薄壳、rstest、termcolor、Windows 控制台依赖链，
也不引入 243/259 代副本。三个组件版本用于来源追溯，不冒称上游新发布。

新组件只供测试使用。`binding-test-tools-validation` 是非生产 normal/build
可达证明入口；它不是 support 测试已经执行的证明。support 的两条边是 dev-dependencies，
不借正式 web-bindings 运行依赖使工具可达。根工作区显式非默认 verification 成员使共享
依赖与测试目标可重复执行。

## 原件与当前改造分离

完整官方 release.crate、registry 精确记录和 194 份原文件保留于中央原件目录，
原始说明在 docs/upstream，许可在 compliance/licenses；原来源名与 VCS 不改。
受审导入计划绑定旧 catalog、旧 layout、原件全量摘要、初始当前树及外置测试逐文件归属。
原件包含未接入的生成脚本、基准和 DWARF 能力，不执行原档脚本，不把未接入能力称为通过。

解析器不纳入 DWARF/gimli 和 component-model；printer 仅保留普通打印，移除 termcolor、folded、component-model、validate/operand-stack 及无人消费的呈现配置/API。Unicode 组件移除 rustc 内部依赖、无效 no_std 开关和 width_cjk/Ambiguous=2 专用算法/表，正常中文宽字符、组合符与 emoji 宽度不变。
具体模块和上游测试处置见三个[组件](../components/wasm-text-parser.md)
[说明](../components/wasm-text-printer.md)及[显示宽度说明](../components/unicode-display-width.md)。

## 已证明的诊断修复

上游 Text::new 先展开 tab/去除方向控制符，后用原字节偏移切渲染字符串；独立回归实际证明
两例错位：tab 后报告列 5 而应为 7，bidi 后列 8 而应为 4。现在先将原 span 前缀做同一
渲染变换，再求 Unicode 显示宽度；保留非法公开 span 的原有回退。复杂度仍为 O(n)，不增加
依赖，不以字节数/字符数替代 Unicode 宽度。原 parse-fail 的 55 份独立 `.err` 不重生成：53 份 core/词法例继续实际执行，2 份 component-only 原件归档，不称全上游测试已通过。

## 验收边界

组件测试、声明特性组合、support 全 lib、正式绑定生成、scope 负向 API、Node 与真实
Chromium 分别提供结果。实际 Chromium 和真实 Samba SDK 仍由相应 CI 运行，不能以 Node
或 Rust 单测代替。不得设置 BLESS 重生成既有预期。

新 printer 的当前边界不沿用上游汇总错误或可插拔输出框架：唯一真实输出是 String，
无单实现 dyn Print、颜色空 hook、IO 伪失败或展示配置。解析错误保留 BinaryReaderError
作为 source；不支持的编码、非法结构和实现上限用具名变体。向 String 的格式化依赖
其不可失败实现，不虚构 fmt/IO 外部错误。新 parser 的 core 是组件本体，不保留必开的
wasm-module 开关、无消费者禁用实现或 Wast 脚本/断言 AST；直接 Wat 与自定义 directive 的实际调用不变。

本轮追加许可通知：

- wast 245.0.1：[MIT](../../compliance/licenses/upstream/wast-245.0.1/LICENSE-MIT)、[Apache](../../compliance/licenses/upstream/wast-245.0.1/LICENSE-APACHE)、[LLVM 例外](../../compliance/licenses/upstream/wast-245.0.1/LICENSE-Apache-2.0_WITH_LLVM-exception)。
- wasmprinter 0.245.1：[MIT](../../compliance/licenses/upstream/wasmprinter-0.245.1/LICENSE-MIT)、[Apache](../../compliance/licenses/upstream/wasmprinter-0.245.1/LICENSE-APACHE)、[LLVM 例外](../../compliance/licenses/upstream/wasmprinter-0.245.1/LICENSE-Apache-2.0_WITH_LLVM-exception)。
- unicode-width 0.2.2：[MIT](../../compliance/licenses/upstream/unicode-width-0.2.2/LICENSE-MIT)、[Apache](../../compliance/licenses/upstream/unicode-width-0.2.2/LICENSE-APACHE)、[COPYRIGHT](../../compliance/licenses/upstream/unicode-width-0.2.2/COPYRIGHT)。

既有 NOTICE 是布局证明固定的历史索引，不改写其字节；当前全部来源及材料另由
[当前来源清单](../THIRD_PARTY_RUST_SOURCES.md)与本轮追加通知完整登记。

## support 测试接线与退役场景

原 87 个逻辑例中，86 个仍有效的逻辑保持；`invoke_redirect` 正向依赖的 Emscripten
描述器跳板已有明确退役来源记录，不能为了凑数恢复。保留其原 WAT，替换为真实
`Bindgen::generate_output` 边界回归：无 marker 的原输入因 describe shim 缺少必需
`__wasm_bindgen_unstable` 元数据而拒绝；两个 Emscripten marker 则在元数据处理之前
明确拒绝。不能宣称仅凭 `env::invoke_*` 名字识别后端，也不能称原 87 个逻辑全部不变。

其余 17 个参数化夹具改为显式普通测试，修正到根 test 的四级路径。解析 helper 留在
原已外置 transforms 测试片段；生产 src 只保留合法 include 接线。去掉 BLESS 写回路径，
19 份原 WAT/golden 的字节逐项不变。`test/ci/fixtures/binding-support-tests.json` 记录原
87 名称、86 保留/1 替代及 19 个摘要，静态库存检查不替代实际 Cargo 发现和执行。

foundation-language 显式执行 support 全 lib 和三个工具的测试；不使用 cfg 隔离残留例、
不以只编译生产 lib、零测试或单个专项代替全套。

## 本轮实际验证（2026-10-26）

在独占 target、jobs=2、关闭 incremental 与 dev/test debug 的本地环境执行：

| 实际入口 | 最终结果 |
| --- | --- |
| support `cargo test --lib -- --list` 与全 lib 运行 | 发现清单逐项吻合；87 通过、0 忽略（86 保留 + 1 退役边界替代） |
| 三个文本工具 `cargo test` | 89 个 harness 项通过；其中 parse-fail 表驱动项实际逐例覆盖 53 份原始 `.err` |
| support 与三个工具 `cargo clippy --all-targets -- -D warnings` | 通过 |
| `manage.py verify --metadata`、architecture/documentation verify | 224 active、107 retired、3 superseded、9 profile 的当前记录通过 |
| 来源工具完整 unittest discover | 964 项通过，含 26 项导入边界与恢复故障回归 |
| `check.sh static` | 420 CI 项、23 perf 工具项及 release shell/evidence-probe 检查通过 |
| `check.sh contracts` | 47 项测试与文档契约通过；2527 本地链接、0 缺失生成物 |
| `check-web-bindgen-scope.py` | 正式 CLI 生成绑定；15 组负向 API、10 项 Node Web-loader 检查通过 |

三个新组件没有保留可选 feature，因此不存在额外声明特性组合。表中工具数不冒充完整
上游套件；Node 也不冒充真实浏览器。实际 Chromium、Samba SDK 验收仍待相应 CI。
静态检查发现并修正了新增 3 来源所需的固定分组数量，以及 HTTP 负向夹具缺少新增导入
证明材料的问题；真实校验仍要求完整材料，并未删去 receipt 标记或放松全集检查。

## 追加边界复核

旧公开假 Component 可以手工构造，encode 实际触发 unreachable panic；已删除该
无职责 AST 与空 DWARF/配置链，改为最短 core-only Wat 包装和文本入口的明确拒绝。
原始红例日志保存为 `binding-parser-component-red.log`；当前不再存在可构造的该变体。

独立审阅曾推断 printer 对字节 `[0,97,115,109,1,0,0,0,10,4,1,2,0,11]` 会 panic，
真实执行否定了该推断：原实现已返回保留 source 的 Parse 错误。wasmparser 的
`Parser::check_function_code_counts` 在返回 CodeSectionStart 前检查 Function/Code
数量；本轮仅增加回归与局部不变量说明，未重复校验或新增此类错误变体。

printer 的其余 unwrap/expect 按调用链复核：eof=true 排除 NeedMoreData；Parser 先发
Version 且拒绝非法段顺序，保证 state；二进制 type reader 仅构造 module index，隐式
RecGroup 恒含一个类型；branch hints 的 pop 由前一 last 匹配保护；try_table 的 label pop
之前必经 block_start push，合法 catch 案例实际通过；名称冲突切片只在第 i-1 项存在时
读取 i 后缀。String 格式化无外部失败，内部静态名称分类与配对文本括号不依赖未验证的
外部枚举构造。未加入完整 Validator、第二遍语义验证或生产 catch_unwind。

本次两文件删除复用 production trim 契约，追加前序 owner 完整快照与精确文件摘要/理由。
仅已认证的新 import owner 可走此路径；旧 grouped owner 的 descendant-chain 禁令仍在，
旧 trim/布局历史投影与初始 import receipt 不修改。新记录验证走该 owner 的直接安全
当前路径，不重新进入历史布局映射；回归覆盖该边界、缺失身份、篡改摘要、恢复文件、
重复删除及原有全部 trim 负向场景。

普通 String 输出的 newline、字符串/字节/名称/类型呈现与 operator/float helpers 已改为
unit（需要参数数量的 helper 返回实际数值），不再保留永远 Ok 的 IO 风格包装。
实际 reader/实现上限错误和 VisitOperator 的真实统一返回约定仍保留；BrTable 的懒解析
错误有独立传播回归，不能为了去问号吞掉 parser 的 finish 检查。

追加后的实际复跑结果：support 87、文本工具 92 个 harness 项（仍含 53 份独立 `.err`）、
combined all-targets Clippy 全通过；来源工具完整 977 项（含 22 项 trim 回归）通过。
metadata、static 420+23、contracts 47 项及 2527 链接、正式 scope 15 组与 Node10 均再通过。
旧 964/89 是前一阶段结果，不代替本次结果；真实 Chromium/SDK 的 CI 边界不变。
