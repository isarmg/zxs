# Web 与 HTTP 基线回归修复

本轮从 `348ff9e89727d96afd865c5a88d6e4ac5299c211` 继续交接。该提交的 [CI 运行](https://github.com/isarmg/zxs/actions/runs/42010398278) 为 12/17 通过，不能作为后续候选已经通过的证据。三个独立问题导致 quality、workspace、foundation-async、web 和浏览器五个 job 失败。本说明只记录这些基线问题及本地验证，不代替本轮引擎变更的验证说明。

## 行为与实现

1. HTTP 日常管理回归仍通过 `Kind::Smb` 调用已移除的 Web 表单。现有 `Kind` 已不表示账户、授权和 SMB 绑定表单；移除此过期表单调用，不恢复退役 UI。SMB HTTP 绑定协议的独立 `optional_identities` 集成回归保留，未删除服务器能力或授权检查。
2. Select 的既有样式契约要求卡片保持固定，仅选项内部滚动，而生产代码仍滚动整个卡片。增加独立 `.zxs-select-options` 滚动层，将键盘活动项定位应用于此层；外卡片保留 `overflow: visible`。前景 SVG 通过局部绝对定位与负边框偏移绑定卡片 border box，不再使用通用 fixed 前景的视口纠偏。未改动选项激活、焦点、关闭、监听器生命周期或浏览器准入规则。固定前景仍使用原有坐标纠偏，并传播 CSS 属性读取错误。
3. 文件行像素观察器以页面底色作为悬浮填充颜色，而产品已使用共享 `--zxs-row-hover-bg` token。观察器现读取该声明式 token，与真实截图像素独立比较；未从已绘制 surface 反推预期，未放宽像素容差或角区检查。模型新增旧白色预期必须失败的负例。

局部前景样式生成只使用已取得的几何与 CSS 快照，不增加 DOM 扫描、事件监听、I/O、锁或同步操作。每个前景仍为常数额外工作和一份样式字符串。选项定位沿用现有单项 DOM 查找，不按选项数量新增遍历。

## 已通过

- 固定 Rust 1.99.0：修改的 Rust 文件 `rustfmt --check`，以及 `git diff --check`。
- `bash .github/scripts/check.sh web`：801 个测试全部通过，包括 24 个样式契约、596 个 Web 单元测试和新增局部前景 border-box 定位单测。host 成功不代表 WASM DOM 或真实浏览器行为已通过。
- `cargo test --locked --offline -p storage-server --test http_engine daily_management`：19 个测试通过，包含真实管理操作与永久回执/边界契约。
- 同次编译所得 `http_engine` 测试二进制执行 `optional_identities::typed_smb_binding_http_preserves_wire_replay_and_error_priority --exact --test-threads=1`：1/1 通过，独立验证仍受支持的 SMB HTTP 协议。
- `python3 .github/scripts/check-rust-diagnostics.py -- cargo check --locked --offline --target wasm32-unknown-unknown --release -p web-app --message-format=json`：通过；包含仅在 WASM 中编译的前景控制器，诊断流无 Rust warning/error。
- `python3 .github/scripts/check-rust-diagnostics.py -- cargo clippy --locked --offline -p web-app -p web-interface -p view-macros -p browser-smoke --all-targets --message-format=json -- -D warnings`：修复引擎候选的三项诊断后复跑通过，包含当前引擎/服务器依赖生产图；无 Rust warning/error。引擎测试目标另行执行，不以依赖编译冒充其全部目标 lint。
- `cargo test --locked --offline -p browser-smoke`：41/41 单元测试通过，包含观察器拒绝错误证据、夹具、HTTP 与生命周期契约，不代表启动了 Chrome。
- `python3 -m unittest discover -s test/ci -p 'test_file_row_decoration.py'`：6 个测试通过。
- 文件行像素模型 60、Select 滚动模型 28、边框模型 45、焦点模型 32 个用例通过。模型不是实际浏览器截图证据。
- `node .github/scripts/check-evidence-probes.mjs`：26 个历史模块的静态、便携配置和清单检查，以及 5 个支持测试通过；该入口不执行浏览器验收。
- 原始基线的隔离副本执行 `bash .github/scripts/check.sh static` 通过。此项验证的是原始基线，不是尚未完成来源元数据刷新的候选树。
- 本轮组合工作树已完成来源清单及索引刷新，`manage.py verify --metadata` 通过：224 个活动源码树、107 个退役、3 个替代来源和 9 个闭包配置；原始身份、来源原文、许可证、历史记录、manifest 与锁文件未改写。
- 刷新后的组合工作树完整 `bash .github/scripts/check.sh static` 重跑通过，包含 697 个 CI 回归、72 个 publication model、23 个 perf-check、68 个 history-scale、49 个 local-RPC 测试，以及发布打包/发布流程模拟和证据探针静态检查。

## 失败、未执行与环境限制

- 基线 source-tools 首次全量执行 1008 个测试，3 个错误均为隔离源码归档缺少 `.git`，使 `git check-attr` 无法读取仓库属性；其余 1005 个无失败。初始化该隔离副本的 Git 元数据后，包含这三项的 `test_external_tests.py` 26/26 通过。未将首次全量记录改写为通过。
- 最终 static 首轮的 `TMPDIR` 被设置在仓库内，触发独立 Cargo 夹具不得位于产品仓库的 11 处断言。改用仓库外临时目录完整重跑通过，未修改夹具位置门槛或源码；首轮失败日志保留。
- 本地 Chromium 的标准沙箱启动在创建 socket 时返回 `Operation not permitted`，且没有 ChromeDriver。未通过关闭浏览器沙箱、改变系统安全设置或跳过实际像素断言制造通过。真实滚轮反向滚动、弹层几何、像素截图、键盘焦点和完整打包浏览器验收仍须由本轮提交的正式 CI 执行。
- 初始环境没有 Rust。用户批准后在隔离工具目录安装官方固定 Rust/Cargo 1.99.0、rustfmt、clippy 和 `wasm32-unknown-unknown`；未改写仓库工具链或使用注册表依赖。
- 相关 all-targets Clippy 首次因引擎候选的 `collapsible_if`、`too_many_arguments` 和 `large_enum_variant` 三项诊断失败；分别通过等价条件合并、具名 `TagPublication` 输入和共享 `Arc<VerifiedReplicaSource>` 修复后复跑通过。未降低 `-D warnings` 或诊断流门槛。
- 本说明不提供全仓 quality/workspace、Samba SDK 与正式打包浏览器矩阵的通过结论。最终来源元数据刷新、static 和正式 CI 按统一集成提交验证，不能由上述定向通过推定。
