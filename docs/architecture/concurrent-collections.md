<a id="concurrent-collections"></a>

# 并发集合

`src/foundation/concurrency/concurrent-collections` 是一个 Rust 包，公开 `utilities`、`epoch`、`queue`、`deque` 四个明确命名空间。Rayon 和 SQLite 访问层使用同一包，不经过兼容别名。

- 无特性：`no_std` 工具原子、退避与缓存行填充。
- `alloc`：存在指针原子时提供 epoch 回收与分配型队列。
- 默认 `std`：作用域线程、同步工具、默认 epoch 收集器与工作窃取双端队列，隐含 `alloc`。

上游空的 `nightly` 兼容特性未选择任何保留实现，已移除；负向 Cargo 解析探针防止重新引入。

各原模块保留自己的静态值、线程局部收集器及原语包装，导入在模块边界内限定。原子顺序、内存屏障、unsafe 操作、所有权/析构及回收算法不变。sanitizer / 无原子构建检测保留；合并构建调用三个保留的探测入口，不静默遗漏任何一个。

<a id="origins"></a>

## 来源

活动主来源为 crossbeam-utils 0.8.23，导入来源为 crossbeam-epoch 0.9.21、crossbeam-queue 0.3.14、crossbeam-deque 0.8.8。均来自 https://github.com/crossbeam-rs/crossbeam，保留 MIT OR Apache-2.0 许可。准确身份、文件路径、保留测试和可逆导入修改由整合收据与中央合规清单登记；原许可证及上游文档不改写。

<a id="validation-and-limits"></a>

## 验证与限制

原先因缺失 `rand` 而休眠的六份集成（`array_queue`、`seg_queue`、`deque` 的 `fifo`／`lifo`／`injector`、`sharded_lock`）改用本地 `fastrand` 取随机步数，由 `check-foundation-behaviour.py` 以逐名清单执行（见[基础组件测试覆盖](../validation/foundation-test-coverage.md)）。Loom 测试源码与配置分支保留，但没有 Loom 提供者，不宣称运行了 Loom 或 Miri。WASM 结果仅为编译检查，不是浏览器线程执行。这些检查不证明穷尽调度，也不证明性能改善。

<a id="maintained-verification"></a>

## 持续验证

运行 `python3 .github/scripts/check-concurrent-collections.py`。基础 CI 调用此门禁，执行原生/WASM 特性构建、分配 API 拒绝、保留单元/集成/文档测试，以及并发、Rayon、析构和窃取回归。临时 manifest 和锁文件不改变根锁或受治理源码字节。队列、双端队列与分片锁的六项保留集成由 `check-foundation-behaviour.py` 执行；Loom、Miri 和穷尽调度仍明确未验证。
