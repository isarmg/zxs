<a id="stable-client-side-reactive-runtime"></a>

# 稳定的客户端响应式运行时

当前浏览器技术栈分为三类职责：

- `reactive-state` 管理响应式依赖跟踪、owner/child 生命周期、清理、signal、记忆化和 effect；没有服务端/客户端共享的 hydration 上下文，也没有热重载函数指针注册表。
- `view-renderer` 管理 DOM 构造、模板克隆、事件绑定、增量重建和卸载。其 `hydration` 游标模块仍有用途：遍历客户端克隆模板，而非服务端渲染的 HTML。
- `dom-helpers` 提供浏览器辅助能力；`src/interface` 组合应用组件与生命周期行为。应用中的浏览器定时器使用回调 API，而非可选的 timer futures 适配器。

工作区固定稳定版 Rust，支持 `.get()`、`.set()` 和稳定闭包转换；不提供 nightly signal 调用简写、服务端 hydration 或 subsecond 热重载模式。响应式图/owner、DOM 渲染和应用组件继续分开，各自具有独立的行为与测试契约。

生产浏览器依赖只开启实际使用的 futures 异步工具、通道和共享状态，不启用其本地执行器、I/O 或 Sink 组合器。任务调度仍走统一浏览器执行器；`reactive-state` 文档测试需要的本地 executor 仅作为开发依赖，不能泄漏到 `web-app` 的正常构建图。CSR 门禁检查真实 WASM 依赖特性闭包。

## 状态读取与派生

`RwSignal` 是 owner 管理的 arena 值，`ArcRwSignal` 是引用计数值。二者都使用 `reactive_state::prelude` 中的 trait 方法：`.get()` 读取并克隆，`.with(...)` 在借用期间读取，`.set(...)` 替换，`.update(...)` 原地修改并通知依赖。不需要建立响应式依赖时，使用相应 `*_untracked` 读取。不要跨 `.await` 保留 read/write guard。

文件管理器磁盘根视图只投影加载／空状态和表格需要的磁盘记录，不复制目录、选择、历史及操作状态；`Select` 显示当前值时借用选项列表，只保留匹配标签，空列表仍不读取选中值。投影在构造 DOM 前结束借用，保持原 owner 与监听器生命周期；这限定了复制范围，不代表已有端到端性能或视觉测量。

文件列表与详情页面分别提供一个共享编辑 Context Memo；行内备注和分支 Draft 只订阅它，不再逐编辑器克隆相同上下文字段及扫描当前目录分支。Memo 仍需在全局 State 更新后比较这份窄投影，但等值时不通知每个编辑器。编辑开始和提交仍同步捕获当前 Context，保持会话 epoch、页面、目录、分支、选择归属和未确认操作处理；共享显示派生不替代请求携带的原始快照，也不改变异步提交栅栏。

同一页面还共享 `Snapshot<Value>` 身份 Memo 和历史 ID→位置索引。标签、版本/路径备注显示及合并来源记录借用同一快照中的记录，不复制节点 Value；保留输入次序、重复 ID 的首次匹配及缺失历史的显示语义。历史分配身份未变时不重建索引，身份变更或清空时整体替换，索引和快照由同一值持有。节点、版本、分支总数为 N 时，建索引 O(N log N)、附加空间 O(N)，每次查询 O(log N)；这消除了这些显示消费者原来的逐节点线性查找，而不是宣称所有版本树计算已线性化。标签内容、显示字符串和合并输入数量仍决定输出工作量。语言依赖仍由实际格式化 Memo 跟踪。

版本树在首次挂载时仍同步测量并尝试一次初始居中；后续 ResizeObserver 与 MutationObserver 通知共用该挂载实例的一次待执行 animation frame，帧回调读取最新 layout 后测量。MutationObserver 仍立即给新节点注册 ResizeObserver，不延迟注册或过滤既有触发源。每帧内通知可合并，但每次实际测量仍遍历全树并计算原有连线，不是增量几何缓存；几何比较、G4 路由、字体、DOM 以及后续不抢用户滚动位置的规则均未改变。调度失败保留具体浏览器错误并回退即时测量。

挂载 owner 清理时先关闭测量状态，再取消待执行帧并断开 observer；帧闭包只持有控制器 Weak，取消失败的迟到回调也不能读取已释放 signal。原生调度状态测试覆盖通知合并、下一帧和清理；真实浏览器仍需检查多源突发、备注编辑、字体/视口变化、卸载重挂、用户滚动和连线最终几何。没有浏览器受控测量时，不将减少重复测量的代码结构描述为已验证的帧耗时改善。

这些派生仅改变计算范围，保留 G4 Automatic、字体、布局和 DOM 行为。原生纯计算/响应式回归不代替浏览器视觉和心跳性能验收；全局 State、其它非 memo 派生及树几何测量仍须分别评审。

派生值优先用闭包或 `Memo`，副作用才用 `Effect`，避免在 effect 中反复写入另一个 signal 造成反馈循环。以下纯计算示例不需要浏览器 DOM 或执行器：

```rust
use reactive_state::{
    computed::Memo,
    owner::Owner,
    prelude::{Get, Set},
    signal::RwSignal,
};

fn reactive_example() {
    let owner = Owner::new();
    owner.with(|| {
        let count = RwSignal::new(1_i32);
        let doubled = Memo::new(move |_| count.get() * 2);
        assert_eq!(doubled.get(), 2);
        count.set(3);
        assert_eq!(doubled.get(), 6);
    });
    owner.cleanup();
}
```

`Effect::new` 需要 `reactive-state/effects` 才调度执行，且首次运行和后续重跑发生在异步执行器 tick，而不是在 `.set()` 内同步结束。`web-interface` 默认 `csr` 特性会开启 effects；直接测试响应式 crate 时必须显式开启并初始化[任务调度器](task-dispatch.md)。

## 挂载、清理与异步任务

[界面挂载 API](../../src/interface/src/mount.rs)在 owner 范围内构造 DOM。`mount_to(parent, app)` 返回必须保留的 `UnmountHandle`，drop 时先卸载 DOM 再执行 owner 清理；`mount_to_body(app)` 将应用根保留到文档结束。需要可重复挂载/卸载的局部界面，不应立即 `.forget()` 或让 handle 提前离开作用域。

owner 清理依次处理子 owner、`on_cleanup` 回调和 arena 值。计时器、DOM listener、observer 等外部资源也要在清理回调中取消或释放；仅移除 DOM 节点不会自动释放应用持有的这些对象。实际例子见 [Web 根组件](../../src/web/src/ui.rs)与[界面组件](../../src/interface/src/components/select.rs)。

弱引用登记与资源清理分开处理：Signal、Memo 和异步派生源在生成通知快照时保序剔除已销毁订阅者，释放源锁后再通知活订阅者；不在每次订阅时扫描整个集合。稳定父 owner 的子登记采用保序、按存活规模摊销的清理阈值，避免列表反复过滤后积累无界死亡登记，也避免逐行创建或替换时重复全表扫描。父 owner 清理后重置阈值。这里保证登记有界与后续通知清理，不承诺所有弱引用在对象销毁的瞬间同步移除。

异步上下文与取消是两个不同契约：

- `reactive_state::spawn_local_scoped` 恢复 owner/observer 上下文，但 owner 清理时**不取消**任务。
- `spawn_local_scoped_with_cancellation` 增加当前 owner 清理时的取消，适合确实隶属于该 owner 的工作。
- 产品 UI 的事件回调使用 [web_interface::tasks::spawn_local](../../src/interface/src/tasks.rs)，绑定到已安装的文档根任务范围，而非某次 render effect；根关闭后不再轮询 UI continuation，避免访问已释放 signal。未挂载时调用会丢弃 future。

上述取消都不证明网络请求已中止、服务器写入已回滚或操作未提交。操作结果和重试必须遵守应用协议。`web-sys` 方法也需在其 manifest 启用对应浏览器类型特性；原生/WASM 编译成功不等于 DOM API 在当前浏览器环境可用。

## 原始文档与验证

[reactive_graph 0.2.16](../upstream/reactive_graph-0.2.16/README.md)说明依赖跟踪模型，[tachys 0.2.19](../upstream/tachys-0.2.19/README.md)提供原始框架背景。前者在这里导入为 `reactive_state`，后者为 `view_renderer`；上游完整 Leptos 的 SSR、hydration、nightly 调用语法及构建工具示例不属于本仓库支持范围。应用 API 以 [web-interface](../components/interface.md) 为入口，不直接照搬上游 `leptos::*` 示例。

CSR 范围门禁通过隔离 manifest，在启用及禁用 effect 两种情况下运行保留的响应式单元测试与本地生命周期回归；typed-builder 仅供测试使用。门禁还检查真实模板探针的原生及 WASM 目标，并拒绝已退役的 owner API 和特性请求。界面集成测试覆盖清理及 parent/weak-owner 生命周期。`test/ci/test_reactive_csr_scope.py` 的静态回归防止退役模式、编译器探测和多余直接依赖请求重新进入。

上游身份、原始 manifest、许可证及源码收据继续保留在 `compliance/provenance` 与 `compliance/rust-sources.json`，不得通过改名暗示这些实现由本项目原创。
