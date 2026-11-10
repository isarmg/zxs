# Futures：异步值、流与任务原语

[组件目录](README.md) · [基础能力](../architecture/foundation-groups.md) · [来源清单](../THIRD_PARTY_RUST_SOURCES.md)

## 编译边界

`src/foundation/concurrency/futures` 提供 future、stream、channel、executor、io、sink、task 和 util 能力。`futures-macro` 仍是独立宿主过程宏目标；它不依赖运行时库，因此不会引入过程宏与目标代码的依赖环。`futures-intrusive` 是独立组件。

名称保留有明确语义的 `futures`，不引入品牌前缀、版本化目录、旧包别名或兼容壳。公共 API 按 `future`、`stream`、`sink`、`task`、`io`、`channel`、`executor` 划分。内部 `traits`、`task_support`、`sink_trait`、`io_traits` 保存实现边界而非重新复制算法。

## 能力与最小依赖

- 禁用默认特性时，只提供无分配的 Future、Stream、Sink、任务与唤醒原语；实际编译图没有其他包。
- `alloc` 增加 boxed trait 对象，`std` 增加标准库 trait 实现；两者均不隐式开启组合器、执行器或过程宏。
- `channel` 开启 alloc oneshot，组合 `std` 提供 mpsc；没有组合器或宏依赖。
- `utilities` 开启组合器及扩展方法，只增加 pin-project-lite。
- `shared-state` 显式开启标准库共享 future 与异步 Mutex，并引入 slab；裸 alloc/std 消费者不负担该存储实现。
- `sink` 开启 utility 的 Sink 扩展，基础 Sink trait 始终存在。`io` 开启 std I/O 扩展及 memchr。
- `async-await` 提供异步辅助；`async-await-macro` 另行开启 futures-macro。导出宏通过 `$crate`/私有公开桥接定位实际库，可以由下游重命名依赖。
- `executor` 开启本地 std 执行器；`thread-pool` 另外开启线程池。WASM 构建通过并不代表浏览器提供原生线程池运行能力。
- 默认能力保留完整 facade 的常用接口。各消费者均设置 `default-features = false` 并按原依赖的实际能力显式转移，包括可选依赖的 feature 转发，避免把某个消费者的需求扩大到所有基础 trait 消费者。

不支持 futures 0.1 / tokio-io 的 `compat`、`io-compat` API；对应原始源码由合规凭据保留。`bilock` 仍要求显式 `unstable`，其余保留 feature 的完整联合须可编译。

## 验证与已知覆盖边界

维护门禁 `.github/scripts/check-futures-consolidation.py` 覆盖 Linux amd64 与 WASM 的最小 feature 矩阵、真实编译依赖图、缺失能力的编译失败、重命名依赖下的宏展开、完整 feature 联合，以及 storage-client/server、platform-io 和浏览器 web-interface 消费者。

门禁运行已接入的单元、集成和文档测试。依赖不可用 futures-test、pin-project、static_assertions 或 assert_matches 的用例、旧兼容测试及 nightly benchmark 不计为执行。原始断言和来源保存在 `test/foundation/futures`，外置来源按 channel/executor/task/util 分类。测试使用临时清单，不增加产品开发依赖或第二套提交的锁文件。

## 来源与许可

八个来源均为 futures-rs 0.3.34，分别保留自己的 MIT/Apache-2.0 许可、上游包标识、原始摘要和外置测试来源。集中许可位于 `compliance/licenses/upstream/futures*-0.3.34`，原文档位于 `docs/upstream/futures*-0.3.34`。合并的是维护与编译边界，不是删除来源或声称这些实现全部原创。
