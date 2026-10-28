# 活动基础组件测试覆盖闭环

[测试布局](../architecture/test-layout.md) · [外置测试来源](../EXTERNAL_TEST_SOURCES.md) · [Rust 来源管理](../tools/rust-sources.md)

本文先给出调用链、分类规则与验收设计，再记录实际执行结果。范围限于 `test/foundation/`、既有测试清单与执行入口；不修改任务 2 的升级／修复适配器，也不改动由固定证明或维护收据约束的 owner。

## 约束

- 测试在**真实消费者解析出的特性**下运行：根工作区 `cargo test --lib -p <包>` 使用根图统一后的特性；临时工作区的依赖特性取自来源清单 `compliance/rust-sources.json` 中生产 profile（`root-native`、`web`、`smb`、`build-tools`）对该包的解析结果。不使用 `--all-features`。
- 不恢复上游包名。保留测试只在两种情况下修改：上游测试引用已退役或不存在的 crate，且本地存在真实替代（`rand` → 本地 `fastrand`，`easy_parallel` → `std::thread::scope`，`pin_utils::pin_mut` → 本地 `futures::pin_mut`）；或接线本身错误。替代不伪造上游 crate，也不新建同名 shim 包。
- 只为本地实现中已不存在的 API 编写的测试，在给出证据后删除，不为测试恢复生产 API。
- 不触及以下 owner 的任何文件：平台族（`tokio`、`synchronization`、`system-entropy`、`libc`、`memchr`、`mio`、`rustc-demangle`）、`ring`、`http-transport`。它们的修改需要新的维护收据或适配器（见[外置测试来源](../EXTERNAL_TEST_SOURCES.md)“整合后的同根维护”一节），不在本任务范围内。
- 编译成功、`ignored`、`filtered out` 或零测试都不算执行。

## 调用链

```
CI foundation 作业
 └─ .github/scripts/check-foundation.sh foundation-async
     └─ python3 .github/scripts/check-foundation-behaviour.py
         ├─ 根工作区单元测试：cargo test --lib -p <包>
         │    src/foundation/<组>/<包>/src/…  #[cfg(test)] #[path]/include!  →  test/foundation/<包>/unit|modules/…
         └─ 临时工作区集成测试（每个套件一个成员包，共用锁文件与 target）
              [dependencies] <组件> = { path = src/foundation/…, default-features = false, features = 生产解析特性 }
              [features]    镜像组件自身特性名，默认启用同一组（保留测试的 cfg(feature = …) 与上游语义一致）
              [[test]] path = test/foundation/<包>/tests/*.rs  或  test/architecture/<包>/behavior.rs
         每个目标：cargo test … -- --list  →  与生成清单逐名比较
                   cargo test …            →  每个列出的测试都报告 ok，0 ignored，0 filtered out
                   日志：target/foundation-behaviour/logs/<套件>.log（含提交 SHA、工具链、命令）
```

覆盖映射由 `tools/rust-sources/foundation_coverage.py` 生成：

```
compliance/rust-sources.json      活动包、各 profile 的目标／角色／特性、已退役记录
cargo metadata / cargo tree       每个一方成员的真实非 dev 依赖路径与按消费者解析的特性
tools/rust-sources/foundation-test-gates.json   已审阅的门禁登记（脚本、命令、执行种类、计数保护）
check-foundation-behaviour.py     本任务套件表（机器读取，不复制）
        ↓
test/ci/fixtures/foundation-test-coverage.json  与  docs/validation/foundation-test-coverage-map.md
```

## 分类

| 类别 | 判定 |
| --- | --- |
| 已执行 | 至少一个门禁运行针对该包的测试：U＝保留上游材料，P＝项目新作契约或夹具，F＝由已执行套件展开的伴随过程宏 |
| 需要但未执行 | 生产 profile 闭包可达，且没有门禁执行针对该包的测试 |
| 仅参考 | 只在验证 profile 或验证成员中可达，没有门禁执行 |
| 已退役 | 来源清单的 `retired`／`superseded` 记录；不得残留 `test/foundation` 根 |

数量全部由生成器从当前仓库计算，不沿用历史报告中的目录计数。“需要但未执行”另按风险分层：并发／平台类且在生产中以运行时角色出现为高，其他运行时为中，仅编译期为低。

## 优先行为与执行方式

| 包（真实消费者） | 行为 | 测试来源 | 适配 |
| --- | --- | --- | --- |
| flume（SQLite 工作线程命令／结果通道，特性 `async`） | 有界背压、全部发送端或接收端释放后的断开、取消挂起的 `send_async`／`recv_async`、断开后取回所有权、零容量交会 | 保留 `basic`、`iter`、`array`、`list`、`zero`；项目新作 `test/architecture/flume/behavior.rs` 覆盖异步取消与断开 | 三个文件中各一个测试的 `rand` → `fastrand` |
| futures-intrusive（SQLite 连接 `sync::Mutex`） | 公平交接、取消排队中的锁请求、守卫释放唤醒、信号量与通道关闭 | 保留 `mutex`、`semaphore`、`manual_reset_event`、`oneshot_channel`、`mpmc_channel`、`state_broadcast_channel`、`timer` 及库内单元测试 | `futures_test` 的计数／恐慌 waker 由项目新作 `tests/support/wakers.rs` 提供；`pin_utils::pin_mut` → `futures::pin_mut` |
| concurrent-collections（SQLite 池空闲连接 `ArrayQueue`） | 有界容量、MPMC 恰好一次、析构释放剩余元素、线性化 | 保留 `array_queue`、`seg_queue`、`deque/{fifo,lifo,injector}`、`sharded_lock`（原 DORMANT） | `rand` → `fastrand` |
| parking（async-notification 阻塞 `wait`） | unpark 先于 park、超时、跨线程唤醒、waker | 保留 `parking.rs` | `easy_parallel` → `std::thread::scope` |
| spin（flume 依赖） | 自旋锁、读写锁、Once、屏障 | 库内单元测试 | 删除只测试本地已不存在的 `Once::init_from_ref` 的一项 |
| signal-hook-registry（tokio signal → 服务端／救援工具关停） | 注册、注销、多处理器 | 库内单元测试与 `unregister_signal.rs` | 无 |
| tempfile（platform-io、storage-client） | 持久化不覆盖、析构清理、spooled 回滚 | 保留五个集成测试 | 无 |
| bytes（SQLite、HTTP、服务端） | `Bytes`/`BytesMut` 分割、冻结、共享 vtable 所有权 | 库内及十个集成测试 | `test_serialization.rs` 需要缺失的 `serde_test`，不执行 |
| slab、hashlink、once_cell、thread_local、scopeguard、guardian、send_wrapper | 键复用、LRU、一次性初始化竞争、线程局部、守卫释放 | 库内或保留集成测试 | 无 |
| flag-set（rustix） | 位标志全集 | 库内单元测试 | 修复 `#[path]` 文件中子模块解析为同级目录导致的 E0583 |

明确不在本任务执行、原因写入生成映射的项目：async-notification 的保留 async-lock 测试（依赖 `futures_lite`、`waker_fn`，本地无等价实现；现有项目契约 `test/async-notification/contract.rs` 继续执行）、tokio 与 http-transport（受维护收据约束）、sharded-slab（`proptest`、`loom`）、futures 的 UNAVAILABLE 列表、rustls 族（缺失夹具）、tower-http。

## 防零测试与改名

- 生成清单 `test/ci/fixtures/foundation-behaviour-tests.json` 记录每个套件目标的完整测试名列表。检查模式下 `--list` 结果必须逐名相等；运行输出中每个名字报告 `ok`，汇总行 `ignored` 与 `filtered out` 均为 0。
- 清单只由 `check-foundation-behaviour.py --write` 根据实际 `--list` 生成，并由最终集成统一提交，不手工编辑。
- `test/ci/foundation-commands.json` 与 `check-foundation.sh` 一致；`test/ci` 单元测试核对套件表、清单与文件存在性。

## 缓存

门禁不按组件源码摘要跳过任何套件，每次都调用 Cargo，由 Cargo 指纹覆盖全部输入（依赖源码、测试文件、特性、锁文件、工具链）。本任务不新增 CI 缓存；若以后缓存 `target/foundation-behaviour`，键必须至少包含 `rust-toolchain.toml`、`Cargo.lock`、门禁脚本、生成清单与 `compliance/rust-sources.json`（已覆盖全部源码与外置测试摘要），不能只看当前组件源码。

## 治理登记

- 修改的保留测试只更新 `compliance/external-tests/foundation.json` 中对应行的 `sha256`，保留 `source` 与 `source_sha256_before`。
- 新文件按项目新作登记：`source_kind: "project-authored"`，`source` 等于 `path`，`source_sha256_before` 等于首次登记的实际摘要。
- 执行 `python3 tools/rust-sources/manage.py refresh`，审查差异后执行 `verify --metadata`。

## 验收

| 命令 | 要求 |
| --- | --- |
| `python3 .github/scripts/check-foundation-behaviour.py` | 全部套件逐名一致并通过，日志含提交 SHA |
| `python3 tools/rust-sources/foundation_coverage.py --check` | 生成映射与当前仓库一致 |
| `python3 -m unittest discover -s test/ci -p 'test_*.py'` | 通过 |
| `python3 tools/rust-sources/manage.py verify --metadata` | 通过 |
| `bash .github/scripts/check-foundation.sh foundation-async` | 在本环境可执行部分通过；需要 WASM 标准库的检查记为环境受限 |

## 实施结果

### 映射与分类

`foundation_coverage.py --write` 在集成提交生成[覆盖映射](foundation-test-coverage-map.md)：活动 222 个包中已执行 94、需要但未执行 110（高／中／低风险 26／47／37）、仅参考 18；来源清单的 107 条退役与 3 条被取代记录均无残留测试根。每个包的消费者与特性来自各 profile 入口的 `cargo tree`，门禁来自已审阅登记 `tools/rust-sources/foundation-test-gates.json`（117 条，去掉了会漂移的行号）与本门禁套件表。登记中引用的脚本必须存在且被 CI 入口调用；`check-rust-syntax-migration.py` 未被任何入口调用，因此未计入。

需要但未执行的高风险项集中在 HTTP／TLS 栈（`rustls`、`rustls-webpki`、`tokio-rustls`、`hyper-rustls`、`tower-http`、`httparse`、`http` 等，缺失夹具或依赖 `serde_test`、`rcgen` 等不存在的提供者）与常数时间原语（`subtle`、`cmov`、`constant_time_eq`）。它们列在映射中，未在本任务执行。

### 行为套件

`check-foundation-behaviour.py` 生成的清单共 20 个套件、1,837 个逐名测试，全部在本地实现上通过；`--list` 名称与清单逐一相等，`ignored`、`filtered out` 均为 0。

| 套件 | 测试数 | 特性来源 |
| --- | ---: | --- |
| flume：保留 `basic`／`iter`／`array`／`list`／`zero` + 项目契约 `behavior` | 96（契约 10） | 生产解析 `async` |
| futures-intrusive：`mutex`、`semaphore`、`manual_reset_event`、`oneshot_channel`、`mpmc_channel`、`state_broadcast_channel`、`timer` | 210 | `alloc, default, std` |
| concurrent-collections：`array_queue`、`seg_queue`、`fifo`、`lifo`、`injector`、`sharded_lock` | 62 | `alloc, default, std` |
| parking | 10 | 无（仅 SQLite 池验证 profile 可达） |
| signal-hook-registry：`unregister_signal` | 1 | 无 |
| tempfile | 64 | `default, system-entropy` |
| bytes（11 个保留集成） | 1,053 | `default, std` |
| slab | 47 | `std` |
| hashlink：`linked_hash_map`、`linked_hash_set`、`lru_cache` | 68 | 无 |
| once_cell：`it`（含 `reentrant_init` 示例进程） | 70 | `alloc, default, race, std` |
| 库内单元：flag-set 54、spin 34、send_wrapper 15、signal-hook-registry 13、futures-intrusive 11、guardian 10、thread_local 9、once_cell 5、scopeguard 3、bytes 2 | 156 | 根工作区解析 |

对保留测试的改动只限：`rand` → 本地 `fastrand`（9 个文件，各一到三处随机步数）、`easy_parallel` → `std::thread::scope`（parking 三处）、`futures_test` 的两个 waker → 项目新作 `tests/support/wakers.rs`、`pin_utils::pin_mut` → `futures::pin_mut`（futures-intrusive 七个文件）、删除 flume 测试中未使用的 `crossbeam_utils` 别名导入、flag-set `tests.rs` 为 33 个子模块写明 `#[path]`（修复 E0583）。spin 删除唯一一个调用本地实现中不存在的 `Once::init_from_ref` 的测试；为执行该测试而恢复此 API 需要新增生产 API，违反测试布局规则。本地仍保留私有的 `force_into_inner`，本次未修改该方法。flume 项目契约中“接收端释放后队列中元素由通道持有，直到最后一个发送端释放”按实际实现断言，这正是 SQLite 工作线程注释依赖的行为。

`check-concurrent-collections.py` 不再保留 DORMANT 列表，原六份集成改由本门禁执行；其余特性矩阵、原有六项集成和消费者检查不变。

### 治理

修改的保留测试只更新了 `compliance/external-tests/foundation.json` 的 `sha256`；`tests/support/wakers.rs` 按项目新作登记。集成时 `manage.py refresh` 暴露了任务 7 的两处违规：集成 owner `src/naming` 的原始生产文件与外置测试行不能删除，`compliance/NOTICE.md` 是布局收据固定的证据。相应文件已原样恢复为未接线材料，NOTICE 恢复原样（见[命名表验证](unicode-naming-tables.md#source-accounting)）。

### 未解决

- async-notification 的保留 async-lock／event-listener 测试依赖 `futures_lite`、`waker_fn`，本地无等价实现，仍只由项目契约执行。
- tokio、http-transport 的保留测试受维护收据约束；sharded-slab（`proptest`、`loom`）、futures 的 UNAVAILABLE 列表、rustls 族与 tower-http 缺失提供者或夹具。
- 既有门禁中接受零测试的 `cargo test` 调用（如 `check-foundation.sh` 的 `--lib -p socket2 -p mio -p synchronization`）未逐一加名称清单；本任务只为新增套件提供逐名保护。
