# Foundation CI 按职责分组

编译、最小特性矩阵和保留测试按职责执行。Foundation 门禁分为五个显式套件；CI 和 Release verification 的
矩阵分别执行各套件，不再把全部 Foundation 工作串在同一个 runner 内。

- `foundation-platform`：Linux ABI、系统后端、系统库单测及小型基础库回归。
- `foundation-crypto`：密码学、系统熵、随机数失败处理和 ring 保留测试。
- `foundation-data`：SQLite、数值类型及精度消费链。
- `foundation-language`：序列化、错误派生、命令行、绑定工具及 Unicode。
- `foundation-async`：运行时任务、futures 行为和 HTTP body 能力边界。

TOML/正则门禁归入 language，futures/并发集合门禁归入 async。新增门禁
必须在其实际集成时加入相应套件和测试库存；不存在的候选检查不会被假装执行，
也不能用“文件存在时才运行”的条件静默跳过。

## 执行与依赖

`bash .github/scripts/check.sh foundation-data` 等命令执行单组。
`bash .github/scripts/check.sh foundation` 仍是本地完整顺序入口，恰好执行五组
的并集。CI 矩阵只列分组，不能再同时列完整入口，以免重复运行。

每组使用同一根 workspace 和 Cargo.lock，Cargo 测试继续使用 locked/offline。
各 runner 保留独立 Cargo home；原 action 固定提交、只读权限和发布流程不变。
当前五组均需要 WASM 标准库，包括 platform 中的 libc 空 Web 接口编译检查。

真实系统 SQLite SDK 仅在 data 矩阵单元安装。其独立 unbundled 消费者在该组后
执行，必须使用系统头文件和 pkg-config，不能将 bundled 测试当作系统库通过。
该需要 SDK 的额外检查不隐式加入本地 portable aggregate；本地有真实 SDK 时可
单独运行 `python3 .github/scripts/check-sqlite-system.py`。

Release 的 `verify` 按套件矩阵运行，`assemble` 必须等待所有 verification 单元。

## 防止遗漏

`test/ci/foundation-commands.json` 是显式命令库存。执行型脚本回归使用录制工具
核对每组的完整参数、所有组的恰好一次并集、非法套件失败和 Cargo 锁定参数。
工作流回归另外核对 CI/Release 矩阵覆盖、无重复 aggregate、SDK 的 data 限定和
Release verification 依赖屏障。新增命令应同时更新脚本和库存，并运行 static。

分组降低单个串行作业的任务量，但不宣称实测耗时、成本或性能改善；实际用时
以对应 GitHub Actions 提交的结果为准。各组仍受工作流原有超时限制。

## 新增门禁

只在对应源码与真实门禁完成集成后，执行下列两项成对修改：

1. 在 `.github/scripts/check-foundation.sh` 的相应 case 中加入一条显式调用。
2. 在 `test/ci/foundation-commands.json` 对应数组中同一位置加入
   `{"tool": "python3", "args": [".github/scripts/实际门禁名.py"]}`。

不得修改只负责分发的 `check.sh` 来注册新家族。
门禁自身的接线测试应读取 `check-foundation.sh`。执行型库存测试会检查参数、
顺序、漏项和重复项。至少运行 `test_check_suites` 与 `test_foundation_workloads`，
再执行该家族自己的门禁和完整 static。

当前完整命令以脚本和库存为准，不另维护候选清单。增加耗时后依据 CI 实测调整分组，不删除必要门禁来规避超时。

绑定工具的 language 接线现在包含 `wasm-bindgen-cli-support --lib` 全测试及三个实际
文本工具的默认测试。工具已收为单一当前能力，不保留无人消费的 feature 矩阵。原逻辑
库存、19份固定WAT/golden及Emscripten退役替代见[专项说明](../validation/wasm-binding-test-tools.md)。
