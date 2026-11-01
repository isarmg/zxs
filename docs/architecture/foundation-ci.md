# Foundation CI 按职责分组

编译、最小特性矩阵和保留测试按职责执行。Foundation 门禁分为五个显式套件；CI 和 Release verification 的
矩阵分别执行各套件，不再把全部 Foundation 工作串在同一个 runner 内。

- `foundation-platform`：Linux ABI、系统后端、系统库单测及小型基础库回归。
- `foundation-crypto`：密码学、系统熵、随机数失败处理和 ring 保留测试。
- `foundation-data`：SQLite、数值类型及精度消费链。
- `foundation-language`：序列化、错误派生、命令行、绑定工具及 Unicode。
- `foundation-async`：运行时任务、futures 行为和 HTTP body 能力边界；`check-foundation-behaviour.py` 统一执行逐名清单保护的基础组件行为测试，包括队列、锁、取消、关闭、所有权转移，以及 `cmov`、`constant_time_eq`、`httpdate` 的限定原生功能测试。密码学与目标覆盖限制见[测试覆盖闭环](../validation/foundation-test-coverage.md)。

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

## 准确基线证据复用（仅 main push）

[选择器](../../.github/scripts/select-ci-suites.py) 只在事件的准确 `before` 提交上，
找到最新一次 main push CI、最新 attempt 的五个准确 Foundation job 全部成功时，
复用输入未变的分片；其它 job 失败不等同于 Foundation 失败。既不搜索祖先或旧 attempt，
也不把 skipped、缺页、网络失败、运行中或缺少 job 当作成功。PR、手动运行、非 main、
未知路径及缺失浅克隆基线均全跑。Release verification 不使用该选择器。

允许的路径不是“不重要文件”：既有 `src/smb-vfs/src/**/*.rs` 内容修改仍运行 language，
`test/smb-vfs/**/*.rs` 只由独立 SDK 目标消费。SMB 文档仅精确允许
`docs/components/smb.md` 与 `docs/validation/debian13-system-samba.md`；既有准确文档
名单不变。构建脚本、manifest、协议包 `src/smb/`、SDK 合同、SDK 脚本、非 Rust fixture
和其它文档均不新增豁免。每次仍运行全部非 Foundation 矩阵、static、SDK、生产 WASM
及真实浏览器/发行候选检查；这不是减少 SMB 验收。

### 消费链依据

审阅入口是 `check-foundation.sh` 的五个 case 及其全部 Python 子命令，不以目录名称
猜测影响。当前源码图（含 runtime/build/dev、目标条件和 optional 边）没有任何包反向
依赖 `smb-vfs`。以下边界绑定到选择器的输入指纹，后续消费者变化必须重新审阅：

| 分片 | 实际消费者及 SMB 边界 |
| --- | --- |
| platform | native-C、UAPI/libc ABI、syscalls、system-backends、parking、synchronization、search-process 各门禁，以及 socket2/mio/synchronization/bit-set/bit-vec/id-arena/rustc-demangle 库测试；唯一第一方编译消费者是 syscalls 的 platform-io，不包含 VFS。临时消费者的真实 Foundation 依赖和根 test fixture 仍在指纹内。 |
| crypto | valgrind-pki、ctutils、crypto-consolidation、linux-random-backends、entropy-consumers、test/entropy/check.py、ring 保留测试及 password-hash；crypto_vectors/crypto_core_checks、backend_environment、foundation_paths 和 ring source-layout/proof-routing 均不读取 VFS 内容。熵文档为 `docs/components/system-entropy.md`，cc 文档为 upstream cc/shlex，不是 SMB 文档。ring localgen 复制全仓 tracked 文件后只运行 ring，因此字节复制不是 VFS 编译；但其 checked_path 拒绝冒号和反斜线，选择器也必须将这些名字判为未知全跑（包括既有 engine/server 路径）。 |
| data | binding-expressions（含 clang）、sqlite-scope/sqlite-access/sqlite-family、numeric-consolidation/precision/numeric、compression-family/date-time-family；真实第一方消费者是 platform-io SQLite 测试及 web-build 测试，不是 VFS。`test/engine/integration/hash_table_compatibility.rs` 是 SQLite 直接输入，继续全跑。CI 额外的 sqlite-system 使用独立 libsqlite3-sys/sqlite-access 消费者，不读取 SMB 源或文档。 |
| language | uuid、form-floats、binding-expressions、foundation-consumers、serialization-family/consolidation、error-definition、command-line、toml、pattern-matching、schema-validation、wasm 文本/绑定工具、unicode-data、url-encoding（含 url-http-consumers）、terminal-style。serialization-family 的 `check_source_paths` 扫描全部 src/tools Rust，所以 VFS 源变动必须保留本片。command-line 实际构建 server/rescue/web-build，URL 检查 client/web，TOML 检查 release-tools/contract-check；这些不反向依赖 VFS。根 VFS 测试不在 src/tools 扫描，也没有被这些消费者包含。 |
| async | task-dispatch、async-notification、tracing、runtime-join-map/features、futures、http-body、concurrent-collections、service-stack、http-routing/transport、foundation-behaviour。futures 的真实 server/client/platform all-targets 编译包含 engine 和 smb-protocol，不包含独立 smb-vfs；所以既有 engine/server 改动仍保留 async。HTTP routing 来源验证的 source-layout/provenance 图不认证活动 VFS 源/测试；不可变来源与 upstream 文档仍在指纹内。 |

以上仅证明允许路径不影响被复用分片，不代表功能、安全或完整 CI 已通过。新的共享测试
引用、依赖边、构建输入或扫描器都需要重新核对；不能只重新生成摘要以消除全跑提示。

Cargo 会解析所有 workspace 成员，即使某个包未被 `-p` 选择。为避免默认 lib/main
入口缺失或自动 target 发现变化，所有已允许的 `src/` Rust 文件新增/删除均全跑，
包括既有 engine/server；只证明内容修改可复用，不假设源文件结构变化无影响。

### 指纹生成与首次全跑

指纹取 Git 树中所有未允许路径的 `[path, mode, kind, blob]`，按路径排序、紧凑 JSON
和换行做 SHA-256；仅排除选择器自身以避免摘要自引用。测试、此说明、workflow、工具链、
锁、manifest、Foundation 和全部未知文件都参与摘要。基线和当前树必须同时匹配同一个
审阅值，任何失配只回退完整 CI。

完成消费者审阅并暂存最终代码、测试和文档后，使用实际最终树生成，而不是猜测值：

```sh
git add .github/scripts/select-ci-suites.py test/ci/test_ci_suite_selection.py docs/architecture/foundation-ci.md
python3 - <<'PY'
import runpy
from pathlib import Path
s = runpy.run_path('.github/scripts/select-ci-suites.py')
root = Path.cwd()
tree = s['git'](root, 'write-tree').decode().strip()
print(s['input_fingerprint'](s['tree'](root, tree)))
PY
```

把输出写入 `REVIEWED_INPUTS` 后再次暂存并重复计算，值应不变。合并其它提交之后须
先审阅新增的非允许输入，再对合并后树重复这一过程；不能从分支直接照抄摘要。
选择器自身或其测试变化都不在允许名单，故引入/调整本机制的 push 必须完整运行，
不能借新摘要复用旧结果。后续合格的小修才可复用准确基线的真实成功证据；连续两次
复用若使准确基线缺少五个实际成功 job，下一次仍全跑，不递归传递成功声明。
