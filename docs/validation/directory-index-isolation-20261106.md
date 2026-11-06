# 目录暂存快照与工作区隔离

本轮基于 `10c3e91ca`，只修复显式目录暂存、提交和状态契约。手动初始化、最近祖先所有权、独立子仓库边界和正常路径读取工作内容沿用现有实现；未加入标准 Git 格式兼容、跨仓库挂载或跨磁盘正文搬迁。

## 行为与类型边界

- 提交读取已审阅的 `index_tree_id`，不再读取最新工作树。暂存 A 后保存 B，提交只记录 A，正常路径仍读取 B；推进 HEAD 不把 B 自动加入暂存区。初始化仍明确捕获用户审阅的现有命名空间作为初始暂存快照，不生成用户提交。
- `DirectoryGitStatus.staged` 与增改删计数比较暂存区和 HEAD；新增必填 `unstaged` 比较工作区和暂存区。两者可以同时为真。严格 Rust DTO、JSON Schema、OpenAPI、正反向向量和 Web 解码同步变更，没有缺失字段默认值或旧格式兼容。
- 只有未暂存内容时，提交返回原 HEAD 的无变更回执。创建、切换分支同时拒绝本仓库的暂存和未暂存更改。历史页的已有 dirty 标记继续覆盖未暂存内容。
- 父提交固定子仓库的相对挂载路径。已引用的子仓库改名后，即使子 HEAD 不变，明确重新暂存也产生待提交的引用变化。整体移动父仓库不会仅因绝对路径变化伪造子引用差异；父自有文件若产生真实路径历史节点，仍正常计为自有内容变化。
- 单独暂存子引用不会暂存父仓库的新工作内容。Web 明确提供暂存按钮和未暂存提示；已暂存 A 与未暂存 B 共存时允许提交 A，并保留 B。

## 成本与同步预算

状态读取增加一次对暂存树的借用索引和工作区／暂存区比较，有直接子仓库时额外扫描暂存树和 HEAD 的目录挂载路径。映射借用条目、上下文和路径，只有有界映射节点及返回身份需要分配；各扫描和比较沿用取消检查、1,000,000 工作步及 16 MiB 字符串预算。没有全树正文复制、无界递归或跨请求缓存。

提交沿用已有不可变树、文件版本引用和事务发布流程。普通保存不增加 Git 发布事务，不改持久化表格式，不减少锁、fsync、鉴权、当前命名空间证明或永久操作回执检查。

## 已通过

工具链为固定 Rust/Cargo 1.99.0，命令使用隔离 `CARGO_TARGET_DIR`、`--locked --offline -j1`。

- `cargo test -p storage-engine --test directory_git_index -- --test-threads=1`：5 项公开 Engine 回归全部通过，覆盖暂存后保存、冷重开、永久幂等、分支防丢失、子挂载改名、父工作内容与子引用独立暂存、父目录移动。
- `cargo test -p storage-engine --test branch_alias_operations --test directory_selection_limits -- --test-threads=1`：43 项分支／别名集成回归和 2 项目录选择预算回归通过。
- `cargo test -p storage-contract directory_git`：严格目录 Git 向量测试通过，包含缺失 `unstaged` 必须拒绝的负例。
- `cargo test -p web-app files::staging::tests`：12 项状态、审阅令牌、按钮和严格解码测试通过。
- `cargo test -p storage-server --test http_engine directory_git:: -- --test-threads=1`：5 项 HTTP 回归通过；真实接口验证暂存 A 后保存 B，提交快照预览为 A、正常文件列表为 B，重开后仍有未暂存状态。
- 复用同次编译的 `http_engine` 二进制运行 `directory_history:: --test-threads=1`：2 项历史读取、暂存查询、鉴权、并发令牌与幂等提交回归通过。
- `cargo test -p browser-smoke -- --test-threads=1`：53 项原生夹具／探针测试通过。子仓库流程现在明确先暂存再提交，预期写请求从 6 次变为 7 次，没有隐式写入。
- 引擎、Web、browser-smoke 全部目标的 Clippy 均以 `--all-targets -- -D warnings` 通过；这覆盖引擎单元测试的类型检查和 lint，不冒充单测已经执行。
- `cargo check -p web-app --target wasm32-unknown-unknown` 通过。此项是 WASM 编译检查，不是 release 制品或真实浏览器验收。
- `contract-check --root .` 通过：2 份 schema、87 条 OpenAPI 路径、476 个 schema／语义向量、3 个规范化向量、2 个校验和向量、5 项严格 JSON 拒绝，文档本地链接无缺失。
- 目录 Git 探针 Python 回归 2 项及其中调用的 Node 回归 5 项通过；维护文档回归 6 项通过；架构边界检查通过。修改 Rust 文件 `rustfmt --check` 与 `git diff --check` 通过。

## 失败、未执行与限制

- 初次尝试运行引擎单元测试时，虽然只筛选目录暂存用例，Cargo 仍需生成整个引擎单测二进制；代码生成阶段收到 SIGKILL，未取得通过结果。改用独立公开 Engine 集成目标完成本轮 5 项行为回归，没有删减现有单测或把生产校验换成测试旁路。
- 首次契约检查指出 OpenAPI 镜像尚未同步，补齐后完整重跑通过。browser-smoke 首次编译发现夹具引用了未声明的契约 crate，改用现有引擎 re-export 后 53 项重跑通过，未新增依赖。
- 父目录移动回归的初始夹具混入了父自有文件，因真实路径历史节点变化而出现自有内容差异。改为仅含独立子仓库的夹具，准确隔离相对挂载路径断言；未修改生产代码掩盖自有文件的合法变化。
- 未执行全仓测试、完整引擎单测运行、真实 Chrome 验收、release WASM 构建、物理磁盘掉电测试或远端 CI。没有推送。上述定向通过不代表这些验证已经完成。
