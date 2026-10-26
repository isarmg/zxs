# 真实浏览器冒烟测试

独立 Rust 工具，直接调用本机 ChromeDriver 的 W3C WebDriver / Chrome DevTools 协议；不引入 Python、Node 或业务 JavaScript，加载真实生产 Web/WASM，不替换页面资源。业务写入使用真实临时服务；下文明确说明的生命周期、分页及宽历史场景会暂缓或构造限定的读取响应，不把合成夹具当作引擎正确性或规模性能证据。少量浏览器端脚本观察错误、已渲染内容和 computed style；独立键盘焦点与绘制回归还临时注入并移除一条装饰故障 CSS，验证真实轮廓控制器的降级行为。

## 依赖与运行

需要 Linux、项目固定 Rust 工具链、匹配的官方 Chrome / ChromeDriver、PATH 中的 `kill`。Chrome 保持默认沙箱，不添加 `--no-sandbox`。GitHub Ubuntu runner 使用已安装的匹配版本；其他环境从官方 Chrome for Testing 下载同版本浏览器与驱动。浏览器或驱动缺失、沙箱禁止启动、页面加载失败均返回非零，不跳过测试。

先按 [Web 界面构建说明](../components/web.md#web-build)构建真实 `build/web`，并构建 CLI：

```sh
cargo build --locked --offline -p storage-server -p storage-rescue
export CHROME_BIN="$(command -v google-chrome)"
export CHROMEDRIVER="$(command -v chromedriver)"
cargo run --locked --offline -p browser-smoke -- \
  --cli-bin "$PWD/target/native/debug/zxs" --web-dir "$PWD/build/web"
```

`CHROME_BIN` 可省略，ChromeDriver 会自动发现浏览器；`CHROMEDRIVER` 默认为 PATH 中的 `chromedriver`。这些变量接受可执行文件完整路径，不接受命令参数。Cargo 单测只验证测试工具自身，不能替代上面的浏览器命令。

## 真实页面截图

可添加 `--screenshot artifacts/web-file-manager.png`：使用产品现有中文语言参数，在测试专用 engine 夹具已持久化、浏览器已加载目录后保存 Chrome WebDriver 返回的原始 PNG。截图只记录实际回归流程中的界面，不额外创建展示文件，不修改页面 DOM、主题或 API 响应；文件元数据、历史和注销验收仍完整执行。截图中使用独立临时开发池，不包含用户文件或凭据。

CI 通过 `ZXS_SMOKE_SCREENSHOT` 传递路径，浏览器验收成功后将图片保存为 `web-screenshot` artifact。README 展示图从该 artifact 原样取用。

## 验收范围

1. 用随机临时密码创建独立三目录开发池，启动真实 ZXS 服务；浏览器登录后必须看到三个磁盘目录。
2. 启动服务前，由 browser-smoke 内部直接调用真实存储引擎，在 P01 的 `browser-fixtures` 命名空间写入同一 UTF-8 文件的两个真实版本。浏览器进入该目录，验证文件行、勾选、搜索、DOM 与焦点保持，不通过已移除的上传入口或 CLI put/get 造数据。夹具只接受工具自己持有的临时目录，拒绝已有仓库；关闭再重新打开引擎后核对两个版本和当前字节。初始化由测试夹具直接调用引擎和认证库；产品 `zxs` 仅作为直接守护进程启动，不调用 `init` 或 `serve` 子命令，测试工具不随产品归档发布。
3. 打开文件详情与版本树，确认两个真实版本；仅在历史 GET 响应中追加 12 个明确标记的合成分叉节点，使桌面视口中的树能够横向滚动。通过原位输入向真实服务提交文件备注，等待真实目录响应的 `selected_entry.note` 和历史刷新，核对树、画布、节点及备注操作控件仍为原 DOM 对象，树的横向滚动位置保持不变。成功或失败均恢复原始 fetch；随后离开并重新打开详情，用未替换的服务响应验证已持久化备注和两个真实版本。合成宽树不写入引擎，也不证明真实大历史的服务端行为。
4. 在操作历史中确认该文件出现。文件页及详情页均断言没有上传、下载、预览、“写入当前分支”、文件输入或传输任务页；网络日志拒绝任何文件正文及写会话请求。元数据 API 与 SMB 协议未改变；普通 CLI 文件传输不是测试前提。
5. 注销并刷新，仍然必须呈现匿名登录页；检查未捕获异常、JavaScript 严重错误、意外 HTTP 错误以及登录、磁盘、历史、注销和 WASM 的成功网络响应。
6. 打开管理员状态页确认真实监测控件仍可用；用真实主题按钮切换明暗模式，并检查普通明暗主题下的键盘焦点：账户密码输入接受 WebDriver Tab 后必须具有可见焦点环；历史筛选的真实 Select 必须完成绘制并可见。普通模式下临时注入 inset shadow，使真实控制器进入不支持装饰的方形 fallback，仍必须可见。注入规则和主题在结束或失败后恢复；不伪造组件状态、API 或轮廓标记。
7. 在真实 WASM 发布启动就绪事件的同一调用栈中注入 `pagehide` 或启动失败事件，验证非缓存离开和失败会同步卸载真实节点、清除 owner 的轮询定时器；缓存离开则保留节点和定时器，并继续完成真实会话恢复。定时器观察器原样调用浏览器原生方法，不替换响应或响应式状态。每次移除注入器，最后再次验证正常导航与刷新。此项覆盖资源释放契约，不声称测量全部堆内存或真实 bfcache 命中率。
8. 暂缓真实 `/api/auth/me` 原始 Response 的交付，在已挂载应用卸载后释放同一个 Response，验证迟到的会话恢复不会访问已释放 owner、重新挂载页面或遗留轮询定时器。原始状态码、响应头和正文保持不变；成功或失败均释放等待并移除注入器。
9. 仅替换目录 GET 响应，构造 205 项分页数据及一次快照失效，验证真实 WASM/DOM 的 200 行上限、前后页替换、本页全选、跨页查询请求、搜索焦点和快照重启；暂缓后故意交付已中止的合成响应，核对新查询与离开目录后的页面不会被旧结果覆盖。成功或失败均释放等待并恢复原始 fetch，最后重新加载真实目录。此项不验证引擎分页排序、快照或规模性能；真实文件行的勾选焦点及服务查询重置另由第 2 项覆盖。

所有数据放在权限受限的临时目录，密码只经 0600 临时文件初始化、不进入命令行。ZXS 仅监听 127.0.0.1；驱动仅允许本机连接。服务、驱动与 Chrome 在同一测试进程生命周期内创建，并通过独立进程组在正常结束和失败时清理；不会修改用户的浏览器配置、现有池或真实磁盘。浏览器配置由 ChromeDriver 为新会话临时创建。失败会输出服务和驱动日志，并将诊断写入 `artifacts/browser-smoke/diagnostics`；临时数据自动删除。

这是小范围贯通测试，不等同于生产磁盘、真实 Samba或完整 UI 回归验收。CI 的 browser-smoke job 必须执行真实浏览器命令，不能将工具编译或单测当作 E2E 通过。

## 会话隔离与退出重试

暂缓真实文件历史的原始成功 Response：有效会话继续后必须显示两版本树；注销完成后才释放的旧响应不得恢复私有文件详情。原始响应正文、状态码和头不变，不注入应用状态。另一个夹具仅中断首次退出传输，验证等待及失败阶段新建目录和文件备注控件禁止新操作、没有业务写请求；显式重试必须真正注销，重新登录恢复元数据写入准入。成功或失败均释放响应及恢复 fetch。

## 浏览器响应式视口截图

设置 `ZXS_VIEWPORT_PREVIEWS=artifacts/viewport-previews` 可保存 1366×900、1440×960、1512×982、412×915、390×844、1024×1366 六个 CSS 视口的原始 Chrome PNG，命名为 `viewport-宽x高.png`。此处“预览”环境变量仅指验收截图，不是产品文件正文预览功能。每张均为真实生产 WASM、真实临时服务上的文件浏览页，使用同一测试专用 engine 夹具，保持真实浏览器 User-Agent。无截图路径时仍执行六视口布局与真实 Tab 焦点检查。

检查页面无横向溢出、元数据工具栏控件可达及可见键盘焦点；文件表格可按产品正常机制内部滚动。检查后清除视口设置，失败也清理。`source.json` 记录来源 commit、工作区是否有改动、实际 Chrome 版本、实际视口和 DPR。工具编译、单测或 JavaScript 语法检查均不能产生或替代实际浏览器结果。

这是单一 Chrome 引擎的响应式尺寸测试，不代表 Safari、Firefox、所有浏览器、手机真机或原生 App 验收。项目不交付原生 App。旧传输任务截图及历史验收报告仅是旧版本证据，不能作为当前 Web 能力说明。

## 字体故障与非阻塞启动

独立回环代理只暂缓原始字体 HTTP 字节或为字体返回 404，其余资源与 API 响应原样转发。真实生产 WASM 必须在两字体仍挂起时发布 ready，登录输入可用；释放原始字体后字体状态独立完成，并解码真实 border-image SVG，核验透明圆角、可见填充和九宫格物理尺寸；紧凑图块按百分比切片与实际圆角范围核对，不错误要求其图像尺寸等于控件尺寸，也不要求不存在的装饰子节点。缺失字体必须保留可用回退界面；挂起期间非缓存 pagehide 后的迟到字体完成不得再写状态或恢复节点。代理在失败和退出时释放等待并关闭，最长等待 90 秒。截图路径仍显式等待字体加载，故障验收在正常截图与业务验收之后执行。测试工具单测只证明代理转发/故障机制，不替代真实 Chrome 验收。

## 版本树帧与生命周期

`test/tools/browser-smoke/runner/tree_frames.rs` 在既有文件备注测试的宽树 fixture 中执行；文件选择、导航、备注保存、会话和请求仍走真实 UI。宽树沿用该测试明确标注的 12 个合成分支叶子，并非后端分支持久化证明。

- `test/tools/browser-smoke/runner/tree_frames.js` 在 WASM 加载前包装宿主 RAF/RO/MO；只把**首个观察目标为版本树 canvas** 的 observer 关联到该组件。G4 的全局 observer 不属于该组件。measure 的证据是 canvas 上精确的 `.zxs-version-node[data-version-id]` 查询，不是全局 `getBoundingClientRect` 计数。
- 先设置窄视口并稳定布局，再点击真实版本备注编辑按钮。输入框和字节计数第二行必须让被观察节点实际增高，且同时收到原生 MO 和该节点的 RO 通知；单纯 viewport resize 不能保证 max-content 画布或达到宽度上限的节点发生 resize。记录原生／人工 delivery 独立计数、目标尺寸、通知尺寸及 pending RAF，超时打印这些诊断。显式暂停组件 RAF，然后对挂载中的两个真实 observer closure 重复 delivery；同一个 JS turn 的八次通知必须不测量、只保留一个 RAF。在原生 RAF turn 中释放它，必须恰好测量一次；检查编辑状态连线后通过真实 Escape 取消，要求原节点高度恢复，再检查最终几何，不提交合成节点 ID。重复 delivery 是测试刺激，不声称自然浏览器必然产生八批通知。
- 通过真实 Files 导航销毁 owner，断言 pending RAF 已取消、两个 observer 已断开。再人为递交已取消的**RAF** callback，断言没有树测量或未捕获错误；不调用已销毁的 observer closure。
- 真实重挂载断言新 canvas、首次测量发生于 observer 注册前、初始 tip 居中；后续测量不得抢用户滚动位置。窄屏、重挂载和桌面恢复均等待实际 `document.fonts.ready` 与渲染帧，然后核对节点宽度/G4 extent/字体、页面溢出、箭头端点/线型及采样路径不穿节点。

边界：这些检查不是屏幕像素差分、全局 G4 算法证明、吞吐基准、延迟字体加载故障注入或全部树拓扑证明。首次同步测量使用唯一 DOM 查询作为代理证据，不能直接读取 Rust 私有 signal；迟到 callback 的失效访问通过抛错、浏览器全局错误及既有最终 console 检查检出。现有 contours/fonts 验收仍独立运行。

## 无浏览器检查

```sh
python3 -m unittest discover -s test/ci -p test_tree_frame_probe.py -v
cargo check --locked --offline -p browser-smoke
```

Node mock-host 自测仅核对探针隔离、暂停/释放和恢复，不运行 WASM，也不能代替 Chrome 验收。Python 用例已进入现有 static suite 的 `test_*.py` discovery。
