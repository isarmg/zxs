# ZXS 浏览器基础层

本 crate 提供 CSR 挂载、主题与语言选择、共享组件和 CSS 驱动的轮廓控制器。业务请求、身份验证及会话状态由应用通过 props 和回调传入组件。

## 支持范围

当前维护浏览器 CSR 路径，默认 `csr` 特性启用 `reactive-state/effects`。界面宏保留 `view!`、`template!` 及普通／透明组件的 props、children 构造；过程宏与界面均由根工作区编译。`Portal` 挂载到常规 DOM，`use_shadow` 必须为 `false`，传入 `true` 会在挂载前明确拒绝。

`view_renderer::view::RenderHtml` 在本项目只负责把浏览器 `<template>` 克隆恢复为可交互视图，静态模板生成由 `ToTemplate` 负责。它保留节点游标、动态值、事件及重建状态，不再提供服务端 HTML 字符串、流、异步服务端解析或服务端 hydration 模式。`ssr`、`hydrate`、`islands`、`mark_branches`、`nightly` 特性已移除；模板恢复无需启用空特性。模板中的 void 元素不生成子占位或闭合标签，动态 style 在恢复时复用正常 DOM 构建路径。

类型化 DOM 接口以 [web-scope.json](../../tools/rust-sources/web-scope.json) 为当前维护范围：42 个 HTML 构造器、17 个 SVG 构造器、55 个属性键及 10 个事件描述符。未使用的 typed MathML、SVG use 和字符串内联事件属性已撤销。自定义元素、Unicode 属性、Custom 事件及 inert HTML/SVG 继续使用真实 DOM 路径；该清单不限制浏览器能解析的标记。web-sys 保留所有当前 manifest（含休眠边）及父类所需的 99 个特性，继续覆盖字体、几何、网络和生命周期。新增接口应先迁移真实调用方并更新此策略及来源收据。


生产构建同时编译独立模板夹具，浏览器套件实际执行 class/style、文本与转义、微任务属性、事件、节点/焦点保留、重建和卸载清理。CI 保存夹具、精确输入身份与浏览器结果，用于将最终提交、来源收据和实际 WASM 对应起来。

宿主回归统一使用 `bash .github/scripts/check.sh web`，覆盖单元、集成及 Rustdoc 测试。过程宏使用仅开发依赖的 `web-interface` 消费者上下文验证当前文档；正常示例编译，`memo!`、`slice!` 示例实际检查响应式读写，错误示例须被拒绝。它不增加发布依赖。浏览器 DOM 行为由真实 Web/WASM 验收单独确认。

`include_view!` 的文件路径相对使用宏的 crate 根目录解析，模板变更作为编译器输入触发重新编译。slot 本身不接收 DOM 事件；事件应放在 slot 内部的元素上，错误用法会直接得到宏诊断。

<a id="modern-browser-contract"></a>

## 现代浏览器契约

只支持满足当前能力契约的现代浏览器，不按浏览器名称或发行版本兜底，不提供旧浏览器 polyfill、前缀替代或缺能力时的原生圆角兼容模式。WebAssembly、ES modules、FinalizationRegistry、WebAssembly.instantiateStreaming 和实际 DOM/SVG 是运行前提；不支持脚本语法／WASM 的浏览器无法运行本应用，资源引导错误不构成兼容实现。

共享 `mount` 在创建组件、主题监听器及字体任务之前执行一次准入：要求 `TextEncoder.prototype.encodeInto`、`ResizeObserver`、`MutationObserver`、`requestAnimationFrame`、`cancelAnimationFrame`、`matchMedia`、`CSS.registerProperty` 与 `CSS.supports`；CSS 要求无前缀的 mask-image URL、clip-path path、`:has()` 和 inline-size container queries。缺失返回包含具体能力名的错误并进入 `failed`，不发布 ready，也不以原生 CSS 圆角继续运行。内部图表和版本树直接使用观察器契约，仍按 owner 清理。此准入是支持下限，不是对全部浏览器行为、CSP 或资源可用性的穷尽证明；实际 API 操作失败仍保留具体错误。

G4 保持 CSS 自动发现的 Automatic 路径及现有 SVG 算法，不切换 CSS `corner-shape`。项目不再实现 forced-colors／high-contrast 专用原生圆角回退、媒体监听和专门配色，也不要求该模式的专项验收；浏览器自身的系统强制色行为不由项目覆盖或保证。普通键盘、焦点管理、语义化 HTML、减弱动态效果偏好和现有布局保持。以下保留原绘制的情形不是旧浏览器兼容：隐藏／零尺寸节点的正常生命周期、作者已有 mask／border-image 的绘制所有权、碎片 inline／table 布局及当前阴影／几何算法限制。它们仍有原有 `data-zxs-contour-*` 诊断，不能与缺 API 的启动拒绝混为一谈。字体资源加载失败、偏好存储被浏览器策略拒绝同样不等于浏览器版本不受支持，既有独立字体和存储语义不变。

浏览器冒烟在真实 WASM 启动前分别移除 ResizeObserver、CSS.registerProperty、mask-image、FinalizationRegistry、instantiateStreaming 和 encodeInto 支持，要求明确失败、没有 ready／登录组件／轮廓降级，并在移除注入后验证正常重载。既有 G4、普通键盘焦点和卸载回归继续保留。仅编译测试夹具不代表这些场景已经实际通过。生成胶水直接使用 FinalizationRegistry 和 streaming，缺能力的导入／初始化错误由资源引导器接收；不在 Rust 挂载处再次探测它们，详见[绑定生成器](../tools/web-bindings.md#modern-web-runtime-contract)。

## 生命周期与公共接口

`mount(root_id, app) -> Result<(), JsValue>` 同步检查根节点并安排异步启动。`i18n::initialize` 与 `theme::initialize` 先应用语言和主题；随后立即挂载应用，不等待字体下载或解码。`ZxsUI` 常规、粗体两个字重在后台加载，保留原字体资源与最终排版。应用挂载并启动轮廓控制器后，先保存完整所有权，再发布 `zxs-startup-ready`。同步错误返回调用者，异步错误发布 `zxs-startup-failed`。

启动状态为 `data-zxs-startup="pending|ready|failed"`。字体状态独立为 `data-zxs-fonts="pending|ready|failed"`：加载中或失败时应用使用系统回退字体且仍可操作，下载完成后浏览器切换到原字体，现有字体完成和尺寸观察器更新轮廓。字体任务收到启动失败事件或离开页面即取消，释放后不再写入字体状态；字体任务不设启动超时。截图验收仍显式等待字体完成。构建工具生成的资源引导器也有 30 秒上限，负责在 WASM 无法加载时用系统字体显示错误页和重新加载按钮。它在动态导入绑定之前安装失败监听器，成功或失败后移除这些临时监听器。`styles/startup.css` 单独提前加载，因此失败提示不依赖应用字体。

应用保留 CSR 卸载句柄和轮廓控制器。非 bfcache 的 `pagehide` 会释放 owner、主题监听器、轮廓观察器、动画帧及 Blob URL；进入 bfcache 时保留页面状态。几何与绘制规则及启动状态转换具有纯 Rust 回归；实际字体资源、浏览器事件和 CSP 仍须通过浏览器验收。

文档生命周期独立于启动结果：`Idle → Starting → Mounted → Closed`，关闭后不接受迟到的挂载结果，同一文档不能重复启动。取消监听在挂载和异步字体工作之前安装，并持续到挂载结束；同步 ready 回调内发生离开或失败时也能找到并释放应用。释放资源时不持有生命周期注册表借用，允许析构期间重入。启动结果 `ready` 不因页面正常离开而改写为 `failed`。

`tasks::spawn_local` 把应用 UI 后续工作绑定到文档根，而非一次渲染的临时 owner；事件回调没有当前 owner 时也能使用。任务完成会移除注册记录，根释放后未完成的 UI future 不再访问已销毁的 signal。取消只表示停止 UI 后续处理，不证明服务器回滚、提交失败或可以重试。

浏览器元数据读取与编辑都受当前文档及会话代次约束；Web 不提供文件上传、下载或正文预览，离开页面后停止 UI 后续处理。已经请求的退出登录传输保留独立异步生命周期以完成服务器会话撤销；完成路径通过非 panic 的状态读取确认 UI 仍有效。

轮廓内部名为 `zxs-g4`。`contours/profile.rs` 按[《给矩形倒圆角 (2)》的 G4 公式](https://zhuanlan.zhihu.com/p/713026877)构造曲率：`κ(u)=Cf(6u⁵−15u⁴+10u³)`，其中 `u=s/L`，再对切向积分求坐标；完整一角由缓和段、中央圆弧及镜像缓和段组成。本项目选取圆弧转角比例 `p=0.4`，归一化最大曲率 `K=1.486024529088757`，物理圆角范围为 `E` 时半径为 `E/K`。这是原文方案的项目参数，不是原文给出的 Apple 默认参数。

坐标使用 512 格 Simpson 积分表与残余区间积分，截断误差单独记录于 `coordinateIntegrationErrorBound`。曲率及其前两阶弧长导数在直线和圆弧接点连续；采样采用平行曲线二阶导数上界控制弦误差，实际 SVG 仍是误差受限的折线。浏览器使用 0.02 CSS px 总预算；四角变换、边框、阴影、裁切及 CSP 绘制沿用统一控制器。页面比例和字体规则见[样式说明](web/public/styles.md)。

轮廓控制器保留通用 CSS 自动发现及先恢复全部原始绘制、统一测量再绘制的顺序；本批未采用只重绘活跃节点的局部优化。非空文本也可能通过 `dir=auto`／`:dir()`／`:has()` 或容器查询激活原先零范围的节点；显式 `--zxs-contour-extent: inherit` 后代可以随祖先动画连续变化而没有自己的过渡事件，背景继承还依赖祖先的原始 CSS。因此不能把“本帧没有自己的事件”等同于“输出不变”。现有几何缓存及绘制缓存继续复用相同参数，不修改 G4 曲率、布局或误差预算。

真实浏览器冒烟新增延迟从零开始的范围过渡、显式范围／背景继承、方向及容器查询、属性启停和移除节点恢复的输出回归；普通节点的查询计数仅作为诊断信息，不声称本批降低了轮廓扫描成本。仅原生测试／WASM 编译不能称为浏览器验证通过。后续优化须先建立完整的候选注册及依赖／失效契约，不能维护零散 CSS 例外名单。

公共接口：

- `i18n::{t, tf, locale, valid_locale, switch_language}`：支持中文与英文，语言切换使用当前 URL 的 `lang` 参数；有效参数写入 `zxs-language`。
- `theme::{current, toggle}`：主题属性为 `data-zxs-theme`，偏好键为 `zxs-theme`；无手动选择时跟随系统主题。
- `contours::{start, ContourController}`：浏览器控制器；纯几何规则位于 `contours::geometry`。
- `components::*`：壳、登录、账户、导航、对话框和反馈组件，详见 [组件契约](interface/src/components.md)。
- `tasks::spawn_local`：文档根所有权下的 UI 异步任务，不提供网络回滚语义。
- `prelude::ForSnapshot`：面向服务器快照的 keyed 列表，稳定 key 保留行 DOM 与 owner，以只读 signal 接收当前行数据；字段须在响应式闭包中读取。相同数据不触发行通知，移除后重新加入产生新行，重复 key 在发布任何行数据前失败。当前用于文件列表的有界服务端页，键包含会话、磁盘、目录及条目／逻辑路径身份，保留行接收更新后的显示字段。分页、全局排序和查询属于业务层，不由此原语执行；翻页和查询主动移除旧页并清除本页选择，不承诺跨查询保留行。浏览器冒烟检查复选框更新保留 DOM／焦点、查询重置选择及输入焦点；另用明确标注的 205 项合成分页响应检查真实 WASM／DOM 中的行数上限、请求中止和迟到响应隔离。这些场景须实际运行后才能记为通过，不构成引擎规模基准，也不执行 Web 上传。设计与验证边界见[文件列表快照与带键行](../architecture/file-list-projections.md)。
