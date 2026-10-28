# zxs Rust Web 构建工具

本工具只构建本项目的 Rust CSR 界面。`build` 执行锁定的离线 Cargo、严格匹配 0.2.129 schema 的 ZXS Web 绑定生成器，再生成静态发行目录；`check` 检查 wasm32，`test` 运行宿主 Rust 测试。

```sh
ZXS_WEB_BINDGEN="$(bash .github/scripts/rust-build-tool.sh wasm-bindgen)"
export ZXS_WEB_BINDGEN
cargo run --locked --offline -p web-build -- build --product-root .
```

`ZXS_WEB_BINDGEN` 指定 CLI，`ZXS_WEB_TARGET_DIR` 指定浏览器编译缓存。默认缓存为 `target/web`，输出为 `build/web`。CLI 来自自维护的 `tools/web-bindings`，仅保留 Web 绑定命令，删除上游测试服务器、其他命令和网络/TLS 依赖；支持库从 `src/foundation` 离线编译。Python 3 在构建前后验证 `compliance/rust-sources.json` 对应的实际文件，清单与统一来源文档也纳入 Web 构建收据。生产构建不需要 Node、npm 或其他项目目录。

正式 Web 使用 `tools/web-build/src/wasm_release.rs` 中的发布配置：体积优化 `s`、fat LTO、符号剥离。即使调用环境带有原生 release profile 参数，也不会将大型函数名段带入浏览器发行物。

发布有独占锁、输入前后摘要比较、完整资源 SHA-256 清单和确定性 WASM gzip；目录根和子项都拒绝符号链接，仅接收普通文件与目录。FIFO、socket 和设备等特殊文件会在读取前失败，避免输入清单扫描阻塞。普通发布失败会恢复旧目录。构建进程退出后才可处理遗留 `build/.web-build-lock`；发现 `build/.web-backup` 时先检查中断状态。两个目录重命名不构成断电事务。

JavaScript 仅由 wasm-bindgen 生成绑定，并由本工具生成通用资源启动文件；文件管理、认证、摘要和业务交互均来自 Rust。

## 必需资源启动边界

`boot.js` 是 HTML head 内、两份静态 stylesheet link 之前的外部经典脚本，不使用 `async`、`defer` 或内联事件处理器。它是唯一启动 owner，在解析器创建两个 link 之前安装捕获阶段 `load`／`error` 监听；即使预加载扫描器已经取回资源，link 节点及其完成事件仍在脚本执行之后出现。`styles/startup.css` 和 `zxs.css` 各自必须收到真实 `load` 才准入，任一 `error` 明确失败并保留资源 URL。`link.sheet` 非空不是加载成功证明，失败请求也可能创建 CSSStyleSheet；不依据规则数量、资源 timing 或延迟重试猜测成功。

绑定模块的动态导入等待 `DOMContentLoaded`；它与原 module script 都在解析完成后执行，但不声称两者相对 DOMContentLoaded 的时序相同。WASM 初始化再等待两个 CSS 准入结果。30 秒启动 deadline 从资源引导器开始运行计时，不因等待 DOM／CSS 而延长；`boot.js` 自身下载失败或停滞不在它尚未执行的计时范围内，不为此建立第二个启动 owner。

错误或 deadline 立即终结启动、统一释放临时监听并取消 Rust lifetime；仅错误面板创建等待 DOM 完成。创建前禁用尚未成功准入的 stylesheet link，解除其解析器 render-blocking；已准入 CSS 保留，不重新请求或在迟到后重新启用样式。原生 `hidden` 属性隐藏根节点，失败提示只要求基本 HTML 可读，不要求字体／样式恢复。迟到资源不能重新挂载或改为 ready；用户自行刷新，已有按钮也只是手动重载页面，不提供自动恢复。该脚本不添加 CSP 内联权限，也不增加第二套启动状态或字体准入。

输入范围由 [`build-inputs.json`](../../tools/web-build/build-inputs.json) 统一列出，构建器和发布验证器使用同一份清单，清单自身也进入输入摘要。构建器拒绝与其编译时版本不同的清单；修改输入定义后必须重建工具和 Web 产物。

<a id="css-release-bundling"></a>

## CSS 发布折叠

`src/web/styles.css` 按发布 URL `/zxs.css` 解释，其中的相对导入映射到 `src/web/public/`，不是入口源码的磁盘父目录。构建器递归展开顶层、位于普通规则之前、以分号结尾的无条件 `@import "path"` 或 `@import url(path)`。每次导入均在原位置展开，包括重复导入；不排序、不全局去重、不压缩，也不删除未用样式、字体或 G4 声明。注释、普通字符串、规则与自定义属性保留原字节；只有真实相对 `url()` 按所属样式表的发布位置重定位到 bundle 根。query／fragment 保留，data、绝对 URL、根相对 URL 和纯 fragment 不改写、不下载。

扫描器识别 CSS 注释、引号与转义、标识符和成对块，不用正则替换搜索到的 `@import` 文本。导入只接受本地路径，拒绝远程／根相对导入、media／supports／layer 条件、规则后的导入及 `@charset`／`@namespace`；这些语法需另行建立无损折叠语义后再扩展，不静默留在结果中。路径按 CSS 转义和百分号编码解析，拒绝越出 public 发布根、编码斜杠／反斜杠及控制字符；所有读取复用普通文件、无符号链接祖先的构建边界。缺失文件、导入环、无效 UTF-8／词法结构、超过 64 层导入或 16 MiB 展开结果均明确失败，不回退到旧复制路径。

本地 URL 在 **CSS 转义解码后、百分号解码前**拒绝首尾 ASCII 空格及控制字符，避免将[浏览器 URL 预处理](https://url.spec.whatwg.org/#concept-basic-url-parser)会去除的空白错误编码为文件名。显式 `%20` 文件名字节不裁剪，内部普通空格仍编码后保留；该拒绝不改变原样输出的 data／绝对／fragment URL。

边界仅覆盖当前资源语法，不是通用 CSS 优化器：拒绝 BOM、空路径段／目录路径，以及可由字符串承载资源的 `image()`／`image-set()`／`src()`。自定义属性中的 data／绝对／根相对／fragment URL 保留；**相对 URL 自定义属性明确拒绝**，因为其 URL 基址由消费 `var()` 的样式表决定，不能按定义文件猜测，见 [CSS Variables 规范](https://www.w3.org/TR/css-variables-1/#defining-variables)。当前产品自定义属性中的三个图标都是 data URL，不触及此限制；普通声明及其 `var()` fallback 的相对 `url()` 仍按消费文件重定位。

源 CSS 和整个 public 目录仍保留；`styles/startup.css` 仍由 HTML 独立提前加载，已加载时提供系统字体错误视图，未成功时不妨碍基本 HTML 失败提示。现有输入目录已覆盖入口、每个导入文件、字体、折叠器和根 `test/tools/web-build` 测试；生成的 `zxs.css`、独立启动 CSS、原样复制的组件 CSS 和字体均进入完整输出 SHA-256 清单，不新增 manifest 格式。输入前后摘要继续阻止构建期间来源变化。

根测试覆盖嵌套与重复导入、循环／缺失／越界／链接拒绝、URL 重定位、注释／字符串／data URL，以及当前全部生产 CSS 的原字节顺序、字体和资源收据；启动回归静态固定脚本／link 顺序、捕获事件准入、DOM 时序、超时终态取消和未完成 CSS 禁用。真实浏览器冒烟另查 CSSOM 无 import、资源观察不再请求子 CSS、原字重加载，以及禁用缓存与预热后启用缓存的主 CSS 失败、startup CSS／两份 CSS 失败和用户按钮重载。字体独立失败仍由原字体冒烟验证，不得误拒应用。测试源码存在不等于已执行；静态取消回归不等于真实 30 秒超时或绘制验收，减少请求依赖关系不等于已测得 RTT、下载字节或启动耗时收益。

`release-package.sh` 打包前重新校验 Web manifest 的身份、完整输出文件集合和 SHA-256，并按源码侧清单重新计算全部输入，不能通过删掉 manifest 中的输入项来绕过。损坏、缺失、额外文件、过期源码、符号链接和特殊文件都会失败；中央法律资料、文档和原生二进制也只接受普通文件。校验后的字节写入独立暂存快照，再从快照生成发布包，避免后续工作目录变化混入归档。此校验不能代替原生编译、浏览器运行或来源审计。
