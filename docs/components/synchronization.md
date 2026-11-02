<a id="synchronization"></a>

# 同步原语

`synchronization` 在同一个包中维护通用锁契约、线程停驻和具体锁原语。来源分别登记在来源目录和不可变来源文档中。

<a id="capability-boundaries"></a>

## 能力边界

- 禁用默认特性时，仅提供 `api`、不依赖外部库的守卫机制以及 `scopeguard`。此配置为 `no_std`，不会启用 libc、线程本地停驻或具体锁。
- `arc_lock` 增加依赖分配的自持有守卫；`atomic_usize` 增加通用可重入锁。两者均不要求 std。
- `parking` 启用 std、底层等待/唤醒队列、Unix 上的 libc、cfg-if 和 smallvec。底层 once-cell 消费者不必启用具体锁。
- `locks` 增加 Mutex、FairMutex、RwLock、ReentrantMutex、Condvar 和 Once，统一使用同一停驻队列。这是默认配置。
- `serialization`、`send_guard` 和 `hardware-lock-elision` 必须显式启用。除非启用 `send_guard`，否则守卫不实现 Send。序列化保持可选，并使用项目中唯一的序列化类型身份。

面向调用方的运行时停驻开关为 `parking`，once_cell 的可选后端开关也为 `parking`。`futures-intrusive` 只在 std 配置下显式启用锁；spin 的通用锁 API 保持 no_std。不再保留兼容门面 crate。

`futures-intrusive` 的借用锁 future `GenericMutexLockFuture` 在底层 `RawMutex: Sync`
且载荷 `T: Send` 时才实现 `Send`，与所借用的 `GenericMutex` 的 `Sync` 条件一致。
载荷不需要 `Sync`；例如 `Cell<i32>` 可以在互斥保护下移动，`Rc<Cell<i32>>` 则不能。
`python3 .github/scripts/check-async-type-contracts.py` 编译这些正反例，并检查具体诊断；
这些编译检查不执行 future，也不替代互斥、取消和唤醒的运行测试。

<a id="safety-and-validation"></a>

## 安全与验证

底层 acquire/release 顺序、公平交接、超时回调和停驻对象地址稳定的生命周期契约保持不变。不提供实验性死锁检测器。

支持的原生目标为 Linux amd64；浏览器 WASM 编译检查不表示支持原子 WASM 运行时。原始 Unix、通用及原子 WASM 停驻源码按各自 cfg 保留；锁 API、具体锁及 parking-core 的独立来源、许可证和可逆源码映射由合规材料维护。

运行 `python3 .github/scripts/check-synchronization.py`，检查原生/WASM 依赖图、缺失 API 拒绝、Send 守卫特性、竞争/公平交接、映射与自持有守卫、递归锁、降级、条件谓词、超时、无中毒语义和 JSON 序列化。锁/核心单元测试可执行；确定性混合调度不等于随机压力、Loom 或 Miri 覆盖。依赖缺失 bincode 或检测器的测试仅保留来源，不计为执行。
