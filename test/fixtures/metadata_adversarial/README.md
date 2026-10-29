# 分页元数据与分段日志对抗回归

固定基线：`13b664e9fd41a356ffb7ed982d21e97abb123626`。本语料只服务真实读取、准入和恢复入口，不是发布协议模型。格式和合法性依据为根编码规范、`docs/adr/0011-paged-namespace-roots-and-segmented-logs.md`、`docs/architecture/namespace-roots-and-segmented-logs.md` 与 `docs/10-transactions-and-recovery.md`。

## 入口与预期

| 测试模块 | 实际调用链 | 独立预期依据 |
| --- | --- | --- |
| namespace | strict JSON → 域哈希认证 → `ObjectLoaderSource` → `NamespaceNode::decode` → `verify_namespace` | ADR 页／索引条数、字节、排序、first_key、计数、等深和路径预算；显式构造的小图可直接手算 |
| segments | `resolve_snapshot` → radix 图 → 全段读取与认证 → `RawState`／准入 | 段 ordinal 连续；事件 sequence 单调不减；回执 seal 条数／逐条规范字节阈值；ID 到全局回执 ordinal 一一对应 |
| budgets | 真实临时文件 → `load_authenticated_object_observed_at` | 文件长度超 64 MiB 必须在正文读取前拒绝；读取批次 ≤64 KiB；缺失、JSON 截断和 digest 不符不能等价为空 |
| recovery | 三成员真实临时池发布 → 全副本损坏 → `Engine::open` | 发布闭包不能在坏根上成功准入或自动重置；原配置、seal、日志、对象等权威字节不变 |

合法对照保留：事件可以同 sequence；回执 sequence 不是全局 ordinal；非根索引允许 unary；单条页可以超过 64 KiB；页边界可依赖编辑历史。测试不要求用全量重建得到原 namespace 根。普通临时目录不代表独立物理盘或真实掉电。

语义坏对象通过正常编码／域哈希助手重算地址，没有关闭哈希验证；字节损坏则保留原引用并检查认证拒绝。预期来自事先选定的变异和规范，不能通过调用被测验证器生成。`namespace/unsorted-page.json` 是可直接加载的两条目最小反例；其它小图及边界记录由已提交测试构造，以免保存 1 MiB 重复填充语料。`segments/corpus.json` 记录固定种子和变异目录。

## 重放与预算

正式执行仅在 GitHub Actions，固定根 `rust-toolchain.toml` 的 Rust 1.99.0；云编辑机不运行 Rust 构建／测试。主测试分支保留正常 CI，专用 workflow 在独立验证分支；载体提交不作为产品祖先。

```sh
bash .github/scripts/initialize-cargo-home.sh
rustup show active-toolchain
rustc --version
cargo test --locked --offline -p storage-engine --lib --no-run --message-format=json > compile.json
python3 test/engine/unit/metadata_adversarial/run.py --compile-messages compile.json --output results
cargo test --locked --offline -p storage-engine --lib -- --skip metadata_adversarial
```

`run.py` 从真实 Cargo 编译消息确定唯一测试 binary，拒绝零测试。每个精确测试独立子进程：地址空间 1 GiB、CPU 30 秒、墙钟 45 秒、单文件 256 MiB、禁 core dump。panic、信号、超时和任意非零退出均失败；不 catch Rust panic，不默认放行失败。它输出逐测试原始日志、完整命令和 `summary.json`。外部资源上限证明受控执行，不能冒充解析器在任何分配前都有内部预算；`verify_namespace` 的读计数结论只覆盖该入口，完整准入另行核对。

随机组合是固定种子的有界回归，不是无限 fuzz：namespace 64 例，segments 64 例。日志打印种子、case、状态与变异。缩减依次保留单一失败变异、删除无关对象／条目／段、把记录缩到 seal 边界、收缩 ordinal／sequence／计数。每次缩减必须重算被修改对象及所有引用祖先的正常域哈希，并保持同一真实入口、错误类别与失败性质。两条目逆序页再删一条会合法，是该性质的最小条目数；seal 反例保留第一条达到阈值后的额外记录。

## 结果边界

实际通过、失败、未执行以绑定精确测试提交的 Actions 原始日志和 `summary.json` 为准；未执行不能写成通过。发现真实缺陷时保留红色回归与最小图，由主任务修复生产。本任务不修改生产验证算法、磁盘格式、依赖或旧验收记录，也不声称整个恢复协议已经证明。
